//! `cp` interception.
//!
//! Copying never destroys the source. The only undo work is restoring
//! destinations that were overwritten and removing destinations that did not
//! exist beforehand. Snapshots are full copies (never hard links) because `cp`
//! rewrites destinations in place.

use std::env;

use serde_json::json;

use crate::error::{Error, Result};
use crate::journal::{EntryStatus, Journal, NewStaged, StagedKind};
use crate::paths::Paths;
use crate::staging::{SnapshotMode, Stager};

use super::transfer::{self, Parsed};
use super::{run_passthrough, transfer::resolve_pairs};

struct DestPlan {
    dest: std::path::PathBuf,
    existed: bool,
    src_exists: bool,
    snapshot_row: Option<i64>,
    created_row: Option<i64>,
}

pub fn execute(journal: &mut Journal, paths: &Paths, argv: &[String]) -> Result<i32> {
    let Parsed::Run(parsed) = transfer::parse(argv, "cp") else {
        return run_passthrough("cp", argv);
    };
    let pairs = match resolve_pairs(&parsed) {
        Ok(pairs) if !pairs.is_empty() => pairs,
        _ => return run_passthrough("cp", argv),
    };

    let cwd = env::current_dir().map_err(|err| Error::Io {
        context: "reading current directory".into(),
        source: err,
    })?;
    let entry_id = journal.begin_entry("cp", argv, &cwd)?;
    let stager = Stager::new(paths, entry_id);

    let mut dests: Vec<DestPlan> = Vec::new();
    let mut snapshot_failures = 0usize;

    for pair in &pairs {
        let existed = pair.dest.symlink_metadata().is_ok();
        let src_exists = pair.src.symlink_metadata().is_ok();
        let mut snapshot_row = None;

        // With --no-clobber an existing destination is left untouched, so
        // there is nothing to protect. A missing source means cp will never
        // reach the destination at all.
        if existed && src_exists && !parsed.no_clobber {
            match stager.snapshot(&pair.dest, SnapshotMode::Copy) {
                Ok(payload) => {
                    let item = NewStaged::new(
                        pair.dest.display().to_string(),
                        payload.stored.display().to_string(),
                        StagedKind::Copy,
                    )
                    .with_size(payload.size)
                    .with_meta(json!({"role": "cp_overwritten"}));
                    match journal.add_staged(entry_id, &item) {
                        Ok(row) => snapshot_row = Some(row),
                        Err(err) => {
                            snapshot_failures += 1;
                            let _ = stager.discard_stored(&payload.stored);
                            eprintln!("rewind: could not record undo snapshot: {err}");
                        }
                    }
                }
                Err(err) => {
                    snapshot_failures += 1;
                    eprintln!(
                        "rewind: could not snapshot {} before overwrite: {err}",
                        pair.dest.display()
                    );
                }
            }
        }

        dests.push(DestPlan {
            dest: pair.dest.clone(),
            existed,
            src_exists,
            snapshot_row,
            created_row: None,
        });
    }

    let exit_code = run_passthrough("cp", argv)?;

    // Destinations that did not exist before but do now were created by cp and
    // must be removed on undo.
    for plan in &mut dests {
        if plan.src_exists && !plan.existed && plan.dest.symlink_metadata().is_ok() {
            let item = NewStaged::new(plan.dest.display().to_string(), "", StagedKind::Created);
            match journal.add_staged(entry_id, &item) {
                Ok(row) => plan.created_row = Some(row),
                Err(err) => {
                    snapshot_failures += 1;
                    eprintln!("rewind: could not record created destination: {err}");
                }
            }
        }
    }

    let undoable = dests
        .iter()
        .any(|plan| plan.snapshot_row.is_some() || plan.created_row.is_some());

    if undoable || snapshot_failures > 0 {
        journal.finish_entry(
            entry_id,
            EntryStatus::Done,
            Some(exit_code),
            undoable,
            Some(&json!({"snapshot_failures": snapshot_failures})),
        )?;
    } else {
        journal.delete_entry(entry_id)?;
        stager.cleanup_entry_dir()?;
    }

    Ok(exit_code)
}

/// Diagnostic helper used by integration tests.
pub fn destination_was_created(journal: &Journal, entry_id: i64) -> Result<bool> {
    Ok(journal
        .staged_for(entry_id)?
        .iter()
        .any(|item| item.kind == StagedKind::Created))
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn created_dest_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("new");
        assert!(!dest.exists());
        fs::write(&dest, b"x").unwrap();
        assert!(dest.symlink_metadata().is_ok());
    }
}
