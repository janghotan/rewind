//! `chmod` interception: snapshot modes, then let the real `chmod` run.

use std::env;

use serde_json::json;

use crate::error::{Error, Result};
use crate::journal::{EntryStatus, Journal, NewStaged, StagedKind};
use crate::paths::Paths;
use crate::staging::Stager;

use super::metadata::{self, Collected};
use super::run_passthrough;

pub fn execute(journal: &mut Journal, paths: &Paths, argv: &[String]) -> Result<i32> {
    let targets = metadata::existing_operands(argv);
    if targets.is_empty() {
        // Let chmod produce its own operand error.
        return run_passthrough("chmod", argv);
    }
    let recursive = metadata::wants_recursive(argv);
    let snapshot = match metadata::collect_modes(&targets, recursive) {
        Ok(Collected::Snapshot(snapshot)) => snapshot,
        _ => return run_passthrough("chmod", argv),
    };
    let encoded = match serde_json::to_vec(&snapshot) {
        Ok(bytes) => bytes,
        Err(_) => return run_passthrough("chmod", argv),
    };

    let cwd = env::current_dir().map_err(|err| Error::Io {
        context: "reading current directory".into(),
        source: err,
    })?;
    let entry_id = journal.begin_entry("chmod", argv, &cwd)?;
    let stager = Stager::new(paths, entry_id);

    let stored = match stager.write_aux("chmod-snapshot.json", &encoded) {
        Ok(path) => path,
        Err(err) => {
            journal.delete_entry(entry_id)?;
            super::warn(&format!("could not save mode snapshot: {err}"));
            return run_passthrough("chmod", argv);
        }
    };
    let item = NewStaged::new(
        targets
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(","),
        stored.display().to_string(),
        StagedKind::Metadata,
    )
    .with_size(encoded.len() as u64)
    .with_meta(json!({"kind": "chmod", "paths": snapshot.entries.len()}));
    if let Err(err) = journal.add_staged(entry_id, &item) {
        let _ = stager.discard_stored(&stored);
        journal.delete_entry(entry_id)?;
        super::warn(&format!("could not record mode snapshot: {err}"));
        return run_passthrough("chmod", argv);
    }

    let exit_code = run_passthrough("chmod", argv)?;
    journal.finish_entry(
        entry_id,
        EntryStatus::Done,
        Some(exit_code),
        true,
        Some(&json!({"paths": snapshot.entries.len(), "recursive": recursive})),
    )?;
    Ok(exit_code)
}
