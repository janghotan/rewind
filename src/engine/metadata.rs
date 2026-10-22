//! Shared metadata snapshot/restore logic for `chmod` and `chown`.
//!
//! A metadata change is cheap to record but tedious to reverse correctly,
//! especially with `-R`. rewind walks the affected tree before the command
//! runs and persists every prior mode (or owner) to a JSON file inside the
//! entry's trash directory. Undo groups paths by their old value and restores
//! them with the real `chmod`/`chown`, batched to stay well under ARG_MAX.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::staging::absolutize;

const MAX_SNAPSHOT_ENTRIES: usize = 200_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModeEntry {
    pub path: String,
    pub mode: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnerEntry {
    pub path: String,
    pub uid: u32,
    pub gid: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModeSnapshot {
    pub recursive: bool,
    pub entries: Vec<ModeEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OwnerSnapshot {
    pub recursive: bool,
    pub entries: Vec<OwnerEntry>,
}

/// Outcome of collecting a snapshot.
pub enum Collected<T> {
    Snapshot(T),
    /// Nothing on disk matched the operands; let the real command explain why.
    Empty,
}

/// Returns true when a short-option cluster asks for recursion.
pub fn wants_recursive(argv: &[String]) -> bool {
    argv.iter().any(|arg| {
        arg == "--recursive"
            || (arg.starts_with('-')
                && !arg.starts_with("--")
                && arg.len() > 1
                && arg.chars().skip(1).any(|c| c == 'R'))
    })
}

/// Every operand that names an existing path. This deliberately ignores mode
/// strings and owner specs: they do not exist as files, so the filter is both
/// simpler and more robust than reimplementing chmod/chown grammars.
pub fn existing_operands(argv: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut only_paths = false;
    for arg in argv {
        if arg == "--" {
            only_paths = true;
            continue;
        }
        if !only_paths && arg.starts_with('-') && arg.len() > 1 {
            // `--reference=FILE` names a path inside the option value; the
            // referenced file itself is not modified.
            continue;
        }
        let candidate = Path::new(arg);
        if candidate.symlink_metadata().is_ok() {
            if let Ok(absolute) = absolutize(candidate) {
                out.push(absolute);
            }
        }
    }
    // Deduplicate while preserving order.
    out.sort();
    out.dedup();
    out
}

pub fn collect_modes(targets: &[PathBuf], recursive: bool) -> Result<Collected<ModeSnapshot>> {
    let mut entries = Vec::new();
    for target in targets {
        #[cfg(unix)]
        {
            // Explicit operands are followed, matching chmod(1).
            if let Ok(meta) = fs::metadata(target) {
                use std::os::unix::fs::PermissionsExt;
                push_mode(&mut entries, target, meta.permissions().mode())?;
            }
            if recursive && target.is_dir() {
                walk(target, &mut |path, meta| {
                    if meta.file_type().is_symlink() {
                        return Ok(());
                    }
                    use std::os::unix::fs::PermissionsExt;
                    push_mode(&mut entries, path, meta.permissions().mode())
                })?;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (target, recursive);
        }
    }
    if entries.is_empty() {
        Ok(Collected::Empty)
    } else {
        Ok(Collected::Snapshot(ModeSnapshot { recursive, entries }))
    }
}

pub fn collect_owners(targets: &[PathBuf], recursive: bool) -> Result<Collected<OwnerSnapshot>> {
    let mut entries = Vec::new();
    for target in targets {
        #[cfg(unix)]
        {
            if let Ok(meta) = fs::symlink_metadata(target) {
                push_owner(&mut entries, target, meta.uid(), meta.gid())?;
            }
            if recursive && target.is_dir() {
                walk(target, &mut |path, meta| {
                    push_owner(&mut entries, path, meta.uid(), meta.gid())
                })?;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (target, recursive);
        }
    }
    if entries.is_empty() {
        Ok(Collected::Empty)
    } else {
        Ok(Collected::Snapshot(OwnerSnapshot { recursive, entries }))
    }
}

#[cfg(unix)]
fn push_mode(entries: &mut Vec<ModeEntry>, path: &Path, mode: u32) -> Result<()> {
    if entries.len() >= MAX_SNAPSHOT_ENTRIES {
        return Err(Error::Staging(format!(
            "refusing to snapshot more than {MAX_SNAPSHOT_ENTRIES} paths"
        )));
    }
    entries.push(ModeEntry {
        path: path.display().to_string(),
        mode: mode & 0o7777,
    });
    Ok(())
}

#[cfg(unix)]
fn push_owner(entries: &mut Vec<OwnerEntry>, path: &Path, uid: u32, gid: u32) -> Result<()> {
    if entries.len() >= MAX_SNAPSHOT_ENTRIES {
        return Err(Error::Staging(format!(
            "refusing to snapshot more than {MAX_SNAPSHOT_ENTRIES} paths"
        )));
    }
    entries.push(OwnerEntry {
        path: path.display().to_string(),
        uid,
        gid,
    });
    Ok(())
}

/// Depth-first walk that does not follow symlinks below the root.
fn walk<F>(root: &Path, visit: &mut F) -> Result<()>
where
    F: FnMut(&Path, &fs::Metadata) -> Result<()>,
{
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
        Err(err) => return Err(crate::error::io_err(root, err)),
    };
    for entry in entries {
        let entry = entry.map_err(|err| crate::error::io_err(root, err))?;
        let path = entry.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(_) => continue,
        };
        visit(&path, &meta)?;
        if meta.file_type().is_dir() {
            walk(&path, visit)?;
        }
    }
    Ok(())
}

/// Restore modes, returning human-readable failures.
pub fn restore_modes(snapshot: &ModeSnapshot) -> Vec<String> {
    let mut groups: std::collections::BTreeMap<u32, Vec<&str>> = Default::default();
    for entry in &snapshot.entries {
        groups
            .entry(entry.mode)
            .or_default()
            .push(entry.path.as_str());
    }
    let mut failures = Vec::new();
    for (mode, paths) in groups {
        for chunk in paths.chunks(256) {
            let mut cmd = Command::new("chmod");
            cmd.arg(format!("{mode:o}")).arg("--").args(chunk);
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Err(err) = cmd.status() {
                failures.push(format!("chmod: {err}"));
                continue;
            }
        }
    }
    failures
}

/// Restore ownership, returning human-readable failures.
pub fn restore_owners(snapshot: &OwnerSnapshot) -> Vec<String> {
    let mut groups: std::collections::BTreeMap<(u32, u32), Vec<&str>> = Default::default();
    for entry in &snapshot.entries {
        groups
            .entry((entry.uid, entry.gid))
            .or_default()
            .push(entry.path.as_str());
    }
    let mut failures = Vec::new();
    for ((uid, gid), paths) in groups {
        for chunk in paths.chunks(256) {
            let mut cmd = Command::new("chown");
            cmd.arg("-h")
                .arg(format!("{uid}:{gid}"))
                .arg("--")
                .args(chunk);
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            match cmd.status() {
                Ok(status) if status.success() => {}
                Ok(status) => failures.push(format!(
                    "chown {uid}:{gid} failed with exit code {}",
                    status.code().unwrap_or(1)
                )),
                Err(err) => failures.push(format!("chown: {err}")),
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    #[test]
    fn collects_and_restores_modes_recursively() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, b"x").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();

        let targets = existing_operands(&["0755".into(), dir.path().display().to_string()]);
        assert_eq!(targets.len(), 1);
        let Collected::Snapshot(snapshot) = collect_modes(&targets, true).unwrap() else {
            panic!("expected snapshot");
        };
        assert!(!snapshot.entries.is_empty());

        fs::set_permissions(&file, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(restore_modes(&snapshot).is_empty());
        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }

    #[test]
    fn recursion_detection_handles_clusters() {
        assert!(wants_recursive(&["-Rv".into(), "x".into()]));
        assert!(wants_recursive(&["--recursive".into()]));
        assert!(!wants_recursive(&["a+r".into()]));
    }

    #[test]
    fn non_path_operands_are_ignored() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, b"x").unwrap();
        let targets = existing_operands(&[
            "u+x".into(),
            "--reference=/nope".into(),
            file.display().to_string(),
        ]);
        assert_eq!(targets, vec![file]);
    }
}
