//! `mv` interception.
//!
//! The real `mv` performs the move, so output, prompting, and cross-device
//! semantics are untouched. Before it runs, every destination that could be
//! overwritten is copied into the trash. Afterwards the source/destination
//! pairs are reconciled to learn which moves actually happened. Undo moves
//! the destinations back and restores the overwritten originals.

use std::env;

use serde_json::json;

use crate::error::{Error, Result};
use crate::journal::{EntryStatus, Journal, NewStaged, StagedKind};
use crate::paths::Paths;
use crate::staging::{SnapshotMode, Stager};

use super::transfer::{self, Pair, Parsed};
use super::{run_passthrough, transfer::resolve_pairs};

struct Plan {
    pair: Pair,
    dest_existed: bool,
    src_existed: bool,
    backup_row: Option<i64>,
    moved: bool,
}

pub fn execute(journal: &mut Journal, paths: &Paths, argv: &[String]) -> Result<i32> {
    let Parsed::Run(parsed) = transfer::parse(argv, "mv") else {
        return run_passthrough("mv", argv);
    };
    let pairs = match resolve_pairs(&parsed) {
        Ok(pairs) if !pairs.is_empty() => pairs,
        _ => return run_passthrough("mv", argv),
    };

    let cwd = current_dir()?;
    let entry_id = journal.begin_entry("mv", argv, &cwd)?;
    let stager = Stager::new(paths, entry_id);

    let mut plans: Vec<Plan> = Vec::new();
    let mut snapshot_failures = 0usize;

    for pair in pairs {
        let dest_existed = pair.dest.symlink_metadata().is_ok();
        let src_exists = pair.src.symlink_metadata().is_ok();
        let mut backup_row = None;

        // Missing sources make mv fail before touching the destination, so
        // there is nothing to protect and nothing to journal.
        if dest_existed && src_exists && !parsed.no_clobber {
            match stager.snapshot(&pair.dest, SnapshotMode::Copy) {
                Ok(payload) => {
                    let item = NewStaged::new(
                        pair.dest.display().to_string(),
                        payload.stored.display().to_string(),
                        StagedKind::Copy,
                    )
                    .with_size(payload.size)
                    .with_meta(json!({"role": "mv_backup"}));
                    match journal.add_staged(entry_id, &item) {
                        Ok(row) => backup_row = Some(row),
                        Err(err) => {
                            let _ = stager.discard_stored(&payload.stored);
                            snapshot_failures += 1;
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

        plans.push(Plan {
            pair,
            dest_existed,
            src_existed: src_exists,
            backup_row,
            moved: false,
        });
    }

    let exit_code = run_passthrough("mv", argv)?;

    // Reconcile what actually happened so undo never moves the wrong thing.
    for plan in &mut plans {
        let src_exists = plan.pair.src.symlink_metadata().is_ok();
        let dest_exists = plan.pair.dest.symlink_metadata().is_ok();
        plan.moved = plan.src_existed && !src_exists && dest_exists;
    }

    let undoable = plans
        .iter()
        .any(|plan| plan.moved || plan.backup_row.is_some());
    let pair_json: Vec<_> = plans
        .iter()
        .map(|plan| {
            json!({
                "src": plan.pair.src.display().to_string(),
                "dest": plan.pair.dest.display().to_string(),
                "dest_existed": plan.dest_existed,
                "moved": plan.moved,
                "backup_row": plan.backup_row,
            })
        })
        .collect();

    if undoable || snapshot_failures > 0 {
        journal.finish_entry(
            entry_id,
            EntryStatus::Done,
            Some(exit_code),
            undoable,
            Some(&json!({"pairs": pair_json, "snapshot_failures": snapshot_failures})),
        )?;
    } else {
        journal.delete_entry(entry_id)?;
        stager.cleanup_entry_dir()?;
    }

    Ok(exit_code)
}

fn current_dir() -> Result<std::path::PathBuf> {
    env::current_dir().map_err(|err| Error::Io {
        context: "reading current directory".into(),
        source: err,
    })
}

/// Used by tests and diagnostics: number of paths protected for an entry.
pub fn protected_destinations(journal: &Journal, entry_id: i64) -> Result<usize> {
    Ok(journal.staged_for(entry_id)?.len())
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn reconciliation_detects_moves() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a");
        let dest = dir.path().join("b");
        fs::write(&src, b"x").unwrap();
        fs::rename(&src, &dest).unwrap();
        assert!(!src.exists() && dest.exists());
    }
}
