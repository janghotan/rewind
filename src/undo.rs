//! Undo execution.
//!
//! Undo is resumable: each successfully restored item is removed from the
//! journal immediately, so a partial failure can be fixed (for example by
//! moving a conflicting file away) and `rewind undo` retried without
//! re-restoring the parts that already worked.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::engine::{git, metadata};
use crate::journal::{Entry, Journal, StagedItem, StagedKind};
use crate::paths::Paths;
use crate::staging::{Stager, move_path, remove_tree};

/// Result of an undo run.
#[derive(Debug, Default)]
pub struct UndoReport {
    pub undone: usize,
    pub failures: usize,
    pub items: usize,
}

/// Undo the last `count` journal entries (newest first).
pub fn run(
    journal: &mut Journal,
    paths: &Paths,
    count: usize,
    dry_run: bool,
) -> crate::error::Result<UndoReport> {
    if count == 0 {
        return Err(crate::error::Error::InvalidInput(
            "n must be at least 1".into(),
        ));
    }
    // Anything left "running" for a day was interrupted (SIGKILL, crash).
    let _ = journal.mark_stale_running(86_400);

    let entries = journal.undoable_entries(count)?;
    let mut report = UndoReport::default();
    if entries.is_empty() {
        println!("rewind: nothing to undo");
        return Ok(report);
    }

    if dry_run {
        for entry in &entries {
            let (items, size) = journal.staged_summary(entry.id)?;
            println!(
                "would undo #{}: {} ({} item{}, {})",
                entry.id,
                entry.command_line(),
                items,
                if items == 1 { "" } else { "s" },
                crate::util::human_size(size)
            );
        }
        return Ok(report);
    }

    for entry in entries {
        let label = format!("#{} {}", entry.id, entry.command_line());
        match undo_entry(journal, paths, &entry) {
            Ok(true) => {
                report.undone += 1;
                println!("rewind: undone {label}");
            }
            Ok(false) => {
                report.failures += 1;
                eprintln!(
                    "rewind: could not fully undo {label}; the remaining items stay in the journal"
                );
                // Preserve stack ordering: do not undo older commands while a
                // newer one is still partially applied.
                break;
            }
            Err(err) => {
                report.failures += 1;
                eprintln!("rewind: could not undo {label}: {err}");
                break;
            }
        }
    }
    Ok(report)
}

/// Undo a single entry. Returns whether it was fully reversed.
fn undo_entry(journal: &mut Journal, paths: &Paths, entry: &Entry) -> crate::error::Result<bool> {
    let stager = Stager::new(paths, entry.id);

    if entry.command == "git_branch_delete" {
        let complete = git::undo_branch_delete(entry)?;
        if complete {
            journal.mark_undone(entry.id)?;
            let _ = stager.cleanup_entry_dir();
        }
        return Ok(complete);
    }

    if entry.command == "git_reset" {
        git::undo_reset(entry)?;
        journal.mark_undone(entry.id)?;
        let _ = stager.cleanup_entry_dir();
        return Ok(true);
    }

    if entry.command == "mv" {
        let success = undo_mv(journal, &stager, entry)?;
        if success {
            journal.mark_undone(entry.id)?;
            let _ = stager.cleanup_entry_dir();
        }
        return Ok(success);
    }

    let staged = journal.staged_for(entry.id)?;
    let mut success = true;

    // Directory entries only describe empty directory structure. Recreate them
    // before files so that file restores land in their original parents.
    if entry.command == "git_clean" {
        restore_clean_directories(journal, entry, &staged, &mut success)?;
    }

    for item in staged.iter().rev() {
        if item.kind == StagedKind::Directory {
            if entry.command == "git_clean" {
                continue;
            }
            eprintln!(
                "rewind: do not know how to restore directory {} for {} (#{})",
                item.origin, entry.command, entry.id
            );
            success = false;
            continue;
        }
        let origin = PathBuf::from(&item.origin);
        let stored = PathBuf::from(&item.stored);
        let result = match (entry.command.as_str(), item.kind) {
            ("rm", StagedKind::Move) => stager.restore(&stored, &origin, false),
            ("cp", StagedKind::Created) => remove_created(&origin),
            ("cp", StagedKind::Copy) => stager.restore(&stored, &origin, true),
            ("chmod", StagedKind::Metadata) | ("chown", StagedKind::Metadata) => {
                undo_metadata(&stager, entry, item)
            }
            ("git_clean", StagedKind::Copy) => stager.restore(&stored, &origin, false),
            ("git_checkout", StagedKind::Copy) | ("git_restore", StagedKind::Copy) => {
                stager.restore(&stored, &origin, true)
            }
            ("git_checkout", StagedKind::Created) | ("git_restore", StagedKind::Created) => {
                remove_created(&origin)
            }
            (command, kind) => Err(crate::error::Error::InvalidInput(format!(
                "do not know how to undo {kind:?} item for {command} (#{})",
                entry.id
            ))),
        };

        match result {
            Ok(()) => {
                journal.remove_staged(item.id)?;
                if !item.stored.is_empty() {
                    let _ = stager.discard_stored(&stored);
                }
            }
            Err(err) => {
                eprintln!("rewind: {}: {err}", origin.display());
                success = false;
            }
        }
    }

    // A checkout/restore may also have rewritten the index.
    if matches!(entry.command.as_str(), "git_checkout" | "git_restore") {
        if let (Some(repo), Some(tree)) = (
            entry.meta.get("repo").and_then(Value::as_str),
            entry.meta.get("index_tree").and_then(Value::as_str),
        ) {
            if let Err(err) = git::undo_index(Path::new(repo), tree) {
                eprintln!("rewind: restoring index: {err}");
                success = false;
            }
        }
    }

    if success && journal.staged_count(entry.id)? == 0 {
        journal.mark_undone(entry.id)?;
        let _ = stager.cleanup_entry_dir();
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Check if any existing component of `origin` starting from `base` is a symlink.
/// Returns `Ok(Some(symlink_path))` if a symlink component was found.
fn check_parent_symlinks(origin: &Path, base: &Path) -> std::io::Result<Option<PathBuf>> {
    let relative = origin.strip_prefix(base).unwrap_or(origin);
    let mut current = base.to_path_buf();
    for comp in relative.components() {
        current.push(comp);
        match current.symlink_metadata() {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Ok(Some(current));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                break;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(None)
}

/// Recreate the empty directories recorded for a `git clean` entry.
///
/// Directories that still exist are left untouched: they either survived the
/// clean (for example because an excluded file was inside) or the user
/// recreated them afterwards. Only missing directories are created.
fn restore_clean_directories(
    journal: &mut Journal,
    entry: &Entry,
    staged: &[StagedItem],
    success: &mut bool,
) -> crate::error::Result<()> {
    let repo_root = entry
        .meta
        .get("repo")
        .and_then(Value::as_str)
        .map(Path::new)
        .unwrap_or_else(|| Path::new(&entry.cwd));

    let mut dirs: Vec<&StagedItem> = staged
        .iter()
        .filter(|item| item.kind == StagedKind::Directory)
        .collect();
    dirs.sort_by_key(|item| Path::new(&item.origin).components().count());

    let mut created: Vec<(i64, PathBuf, Option<u32>)> = Vec::new();
    for item in dirs {
        let origin = PathBuf::from(&item.origin);
        match check_parent_symlinks(&origin, repo_root) {
            Ok(Some(symlink)) => {
                eprintln!(
                    "rewind: cannot restore directory {}: {} is a symlink",
                    origin.display(),
                    symlink.display()
                );
                *success = false;
                continue;
            }
            Ok(None) => {}
            Err(err) => {
                eprintln!("rewind: inspecting {}: {err}", origin.display());
                *success = false;
                continue;
            }
        }

        let created_by_rewind = item
            .meta
            .get("created_by_rewind")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        match origin.symlink_metadata() {
            Ok(meta) if meta.file_type().is_dir() => {
                // If the directory was recreated by the user (or survived the original
                // clean), do NOT change its permissions or touch its contents.
                if !created_by_rewind {
                    journal.remove_staged(item.id)?;
                    continue;
                }

                // If rewind created this directory during a previous attempt, apply
                // the original recorded permissions now.
                let mut mode_ok = true;
                if let Some(mode) = item
                    .meta
                    .get("mode")
                    .and_then(Value::as_u64)
                    .map(|mode| mode as u32)
                {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let current_mode = meta.permissions().mode() & 0o7777;
                        if current_mode != mode {
                            if let Err(err) =
                                fs::set_permissions(&origin, fs::Permissions::from_mode(mode))
                            {
                                eprintln!(
                                    "rewind: setting permissions on {}: {err}",
                                    origin.display()
                                );
                                *success = false;
                                mode_ok = false;
                            }
                        }
                    }
                    #[cfg(not(unix))]
                    let _ = mode;
                }
                if mode_ok {
                    journal.remove_staged(item.id)?;
                }
            }
            Ok(_) => {
                eprintln!(
                    "rewind: cannot restore directory {}: a non-directory entry is in the way",
                    origin.display()
                );
                *success = false;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match fs::create_dir_all(&origin) {
                    Ok(()) => {
                        let mode = item
                            .meta
                            .get("mode")
                            .and_then(Value::as_u64)
                            .map(|mode| mode as u32);

                        // Persist that rewind created this directory so that a retry
                        // can distinguish it from a user-recreated directory.
                        let mut updated_meta = item.meta.clone();
                        if let Some(obj) = updated_meta.as_object_mut() {
                            obj.insert("created_by_rewind".to_string(), json!(true));
                        }
                        if let Err(err) = journal.set_staged_meta(item.id, &updated_meta) {
                            eprintln!("rewind: recording directory state: {err}");
                            *success = false;
                        }

                        created.push((item.id, origin, mode));
                    }
                    Err(err) => {
                        eprintln!("rewind: creating {}: {err}", origin.display());
                        *success = false;
                    }
                }
            }
            Err(err) => {
                eprintln!("rewind: inspecting {}: {err}", origin.display());
                *success = false;
            }
        }
    }

    // Apply modes once every directory exists; traverse bottom-up so restrictive
    // parent permissions do not prevent setting permissions on children.
    for (item_id, path, mode) in created.into_iter().rev() {
        let mut mode_ok = true;
        if let Some(mode) = mode {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(err) = fs::set_permissions(&path, fs::Permissions::from_mode(mode)) {
                    eprintln!("rewind: setting permissions on {}: {err}", path.display());
                    *success = false;
                    mode_ok = false;
                }
            }
            #[cfg(not(unix))]
            let _ = (path, mode);
        }
        if mode_ok {
            journal.remove_staged(item_id)?;
        }
    }
    Ok(())
}

fn remove_created(origin: &Path) -> crate::error::Result<()> {
    let meta = match origin.symlink_metadata() {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(crate::error::io_err(origin, err)),
    };
    let _ = meta;
    remove_tree(origin)
}

fn undo_metadata(
    stager: &Stager<'_>,
    entry: &Entry,
    item: &crate::journal::StagedItem,
) -> crate::error::Result<()> {
    let stored = PathBuf::from(&item.stored);
    let bytes = fs::read(&stored).map_err(|err| crate::error::io_err(&stored, err))?;
    let failures = match entry.command.as_str() {
        "chmod" => {
            let snapshot: metadata::ModeSnapshot =
                serde_json::from_slice(&bytes).map_err(|err| {
                    crate::error::Error::Staging(format!("corrupt mode snapshot: {err}"))
                })?;
            metadata::restore_modes(&snapshot)
        }
        "chown" => {
            let snapshot: metadata::OwnerSnapshot =
                serde_json::from_slice(&bytes).map_err(|err| {
                    crate::error::Error::Staging(format!("corrupt owner snapshot: {err}"))
                })?;
            metadata::restore_owners(&snapshot)
        }
        other => {
            return Err(crate::error::Error::InvalidInput(format!(
                "unexpected metadata undo for {other}"
            )));
        }
    };
    if failures.is_empty() {
        let _ = stager.discard_stored(&stored);
        Ok(())
    } else {
        for failure in &failures {
            eprintln!("rewind: {failure}");
        }
        Err(crate::error::Error::Staging(
            "some metadata restores failed".into(),
        ))
    }
}

fn undo_mv(
    journal: &mut Journal,
    stager: &Stager<'_>,
    entry: &Entry,
) -> crate::error::Result<bool> {
    let mut meta = entry.meta.clone();
    let mut pairs = meta
        .get("pairs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let staged = journal.staged_for(entry.id)?;
    let mut success = true;

    for index in (0..pairs.len()).rev() {
        let src = PathBuf::from(
            pairs[index]
                .get("src")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let dest = PathBuf::from(
            pairs[index]
                .get("dest")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let moved = pairs[index]
            .get("moved")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        if moved {
            let dest_exists = dest.symlink_metadata().is_ok();
            let src_exists = src.symlink_metadata().is_ok();
            if dest_exists {
                if src_exists {
                    eprintln!(
                        "rewind: cannot move {} back to {}: source path is occupied",
                        dest.display(),
                        src.display()
                    );
                    success = false;
                    continue;
                }
                match move_path(&dest, &src, false) {
                    Ok(()) => {
                        pairs[index]["moved"] = json!(false);
                        persist_meta(journal, entry.id, &mut meta, &pairs)?;
                    }
                    Err(err) => {
                        eprintln!("rewind: moving {} back: {err}", dest.display());
                        success = false;
                        continue;
                    }
                }
            } else if src_exists {
                // A previous partial undo already moved it back.
                pairs[index]["moved"] = json!(false);
                persist_meta(journal, entry.id, &mut meta, &pairs)?;
            } else {
                eprintln!(
                    "rewind: cannot restore {}: both the source and destination are missing",
                    src.display()
                );
                success = false;
                continue;
            }
        }

        if let Some(row_id) = pairs[index].get("backup_row").and_then(Value::as_i64) {
            if let Some(item) = staged.iter().find(|item| item.id == row_id) {
                match stager.restore(Path::new(&item.stored), &dest, true) {
                    Ok(()) => {
                        journal.remove_staged(row_id)?;
                        pairs[index]["backup_row"] = Value::Null;
                        persist_meta(journal, entry.id, &mut meta, &pairs)?;
                    }
                    Err(err) => {
                        eprintln!("rewind: restoring {}: {err}", dest.display());
                        success = false;
                    }
                }
            } else {
                pairs[index]["backup_row"] = Value::Null;
            }
        }
    }

    let remaining = pairs.iter().any(|pair| {
        pair.get("moved").and_then(Value::as_bool).unwrap_or(false)
            || pair.get("backup_row").and_then(Value::as_i64).is_some()
    });
    Ok(success && !remaining)
}

fn persist_meta(
    journal: &mut Journal,
    entry_id: i64,
    meta: &mut Value,
    pairs: &[Value],
) -> crate::error::Result<()> {
    meta["pairs"] = Value::Array(pairs.to_vec());
    journal.set_entry_meta(entry_id, meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{NewStaged, StagedKind};

    #[test]
    fn clean_directories_preserves_user_recreated_directory_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let user_dir = tmp.path().join("user_dir");
        std::fs::create_dir(&user_dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&user_dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        }

        let mut j = Journal::open_memory().unwrap();
        let entry_id = j
            .begin_entry("git_clean", &["clean".into(), "-fd".into()], tmp.path())
            .unwrap();

        // Staged item records 0o700 from before deletion, but created_by_rewind is false
        let item = NewStaged::new(user_dir.display().to_string(), "", StagedKind::Directory)
            .with_meta(json!({"role": "git_clean", "mode": 0o700}));
        j.add_staged(entry_id, &item).unwrap();

        let staged = j.staged_for(entry_id).unwrap();
        let entry = j.entry(entry_id).unwrap().unwrap();
        let mut success = true;

        restore_clean_directories(&mut j, &entry, &staged, &mut success).unwrap();

        assert!(success);
        // The user's permissions must remain 0o750, not changed to 0o700!
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let current = std::fs::symlink_metadata(&user_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o7777;
            assert_eq!(current, 0o750);
        }
        // Staged item is consumed
        assert_eq!(j.staged_count(entry_id).unwrap(), 0);
    }

    #[test]
    fn clean_directories_resumes_when_created_by_rewind_and_permission_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let mut j = Journal::open_memory().unwrap();
        let entry_id = j
            .begin_entry("git_clean", &["clean".into(), "-fd".into()], tmp.path())
            .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let read_only_dir = ["/etc", "/usr", "/bin"].iter().map(Path::new).find(|p| {
                p.is_dir()
                    && std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).is_err()
            });

            if let Some(target) = read_only_dir {
                // Staged item with created_by_rewind: true
                let item = NewStaged::new(target.display().to_string(), "", StagedKind::Directory)
                    .with_meta(
                        json!({"role": "git_clean", "mode": 0o700, "created_by_rewind": true}),
                    );
                let item_id = j.add_staged(entry_id, &item).unwrap();

                let staged = j.staged_for(entry_id).unwrap();
                let entry = j.entry(entry_id).unwrap().unwrap();
                let mut success = true;

                restore_clean_directories(&mut j, &entry, &staged, &mut success).unwrap();

                // Because set_permissions failed on read-only system path:
                assert!(!success);
                // Staged item MUST NOT be consumed!
                assert_eq!(j.staged_count(entry_id).unwrap(), 1);

                // Now simulate retrying after permissions can be applied
                let writable_dir = tmp.path().join("now_writable");
                std::fs::create_dir(&writable_dir).unwrap();
                j.remove_staged(item_id).unwrap();
                let writable_item = NewStaged::new(
                    writable_dir.display().to_string(),
                    "",
                    StagedKind::Directory,
                )
                .with_meta(json!({"role": "git_clean", "mode": 0o700, "created_by_rewind": true}));
                j.add_staged(entry_id, &writable_item).unwrap();

                let staged = j.staged_for(entry_id).unwrap();
                let mut success = true;
                restore_clean_directories(&mut j, &entry, &staged, &mut success).unwrap();

                assert!(success);
                let current = std::fs::symlink_metadata(&writable_dir)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777;
                assert_eq!(current, 0o700);
                assert_eq!(j.staged_count(entry_id).unwrap(), 0);
            }
        }
    }
}
