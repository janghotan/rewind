//! Staging: move deleted data into the trash, snapshot data that will be
//! overwritten, and put it back on undo.
//!
//! Design notes
//! ------------
//! * Deletions use `rename(2)` into the trash: O(1), atomic, metadata-exact.
//!   When the trash lives on another filesystem we fall back to a recursive
//!   copy followed by deleting the source.
//! * Overwrite snapshots must be real copies, never hard links: `cp` and `mv`
//!   may truncate and rewrite the destination in place, which would mutate a
//!   hard-linked "snapshot" through the shared inode.
//! * `git clean` snapshots may use hard links because git unlinks the original
//!   paths; that keeps large untracked trees cheap to protect.
//! * Nothing here ever overwrites an existing undo destination by accident.
//!   Overwriting is opt-in per call site.

use std::cell::Cell;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::error::{Error, IoContext, Result};
use crate::paths::Paths;

/// How to snapshot a path that stays in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotMode {
    /// Full recursive copy; safe when the original may be rewritten in place.
    Copy,
    /// Hard-link regular files where possible; safe only when the original
    /// will be unlinked (never rewritten in place).
    Hardlink,
}

/// Result of staging one path.
#[derive(Debug, Clone)]
pub struct StagedPayload {
    /// Where the data now lives inside the trash.
    pub stored: PathBuf,
    /// Approximate number of bytes protected.
    pub size: u64,
}

/// Per-command staging helper. Each intercepted command gets its own
/// `trash/<entry_id>/` directory, which makes cleanup and inspection trivial.
pub struct Stager<'a> {
    paths: &'a Paths,
    entry_id: i64,
    sequence: Cell<u64>,
}

impl<'a> Stager<'a> {
    pub fn new(paths: &'a Paths, entry_id: i64) -> Self {
        Self {
            paths,
            entry_id,
            sequence: Cell::new(0),
        }
    }

    pub fn entry_id(&self) -> i64 {
        self.entry_id
    }

    /// Directory holding all payloads for this entry.
    pub fn entry_dir(&self) -> PathBuf {
        self.paths.entry_dir(self.entry_id)
    }

    fn ensure_entry_dir(&self) -> Result<PathBuf> {
        let dir = self.entry_dir();
        fs::create_dir_all(&dir).path_context(&dir)?;
        Ok(dir)
    }

    /// Generate a collision-free name inside the entry directory.
    fn next_path(&self, base: &OsStr) -> PathBuf {
        let seq = self.sequence.get() + 1;
        self.sequence.set(seq);
        let mut name = format!("{seq:04}-");
        let base = base.to_string_lossy();
        let sanitized: String = base
            .chars()
            .map(|c| if c == '/' || c == '\0' { '_' } else { c })
            .take(120)
            .collect();
        if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
            name.push_str("item");
        } else {
            name.push_str(&sanitized);
        }
        self.entry_dir().join(name)
    }

    /// Move `origin` into the trash (used by `rm`).
    pub fn stage_move(&self, origin: &Path) -> Result<StagedPayload> {
        self.ensure_entry_dir()?;
        self.guard_not_ancestor_of_trash(origin)?;
        let base = origin.file_name().unwrap_or_else(|| OsStr::new("item"));
        let stored = self.next_path(base);
        let size = tree_size(origin)?;

        match fs::rename(origin, &stored) {
            Ok(()) => Ok(StagedPayload { stored, size }),
            Err(err) if is_cross_device(&err) => {
                copy_tree(origin, &stored)?;
                remove_tree(origin)?;
                Ok(StagedPayload { stored, size })
            }
            Err(err) => Err(crate::error::io_err(origin, err)),
        }
    }

    /// Copy `origin` into the trash while leaving it in place.
    pub fn snapshot(&self, origin: &Path, mode: SnapshotMode) -> Result<StagedPayload> {
        self.ensure_entry_dir()?;
        self.guard_not_ancestor_of_trash(origin)?;
        let base = origin.file_name().unwrap_or_else(|| OsStr::new("item"));
        let stored = self.next_path(base);
        let size = match mode {
            SnapshotMode::Copy => copy_tree(origin, &stored)?,
            SnapshotMode::Hardlink => link_tree(origin, &stored)?,
        };
        Ok(StagedPayload { stored, size })
    }

    /// Write bytes (a metadata snapshot or plan) into the entry directory.
    pub fn write_aux(&self, name: &str, bytes: &[u8]) -> Result<PathBuf> {
        self.ensure_entry_dir()?;
        let path = self.next_path(OsStr::new(name));
        fs::write(&path, bytes).path_context(&path)?;
        Ok(path)
    }

    /// Move a staged payload back to `origin`.
    ///
    /// With `overwrite == false` an existing destination is refused, which is
    /// the right behavior for undo of `rm` and `git clean`. Overwrite is used
    /// when restoring a snapshot over a file the wrapped command rewrote.
    pub fn restore(&self, stored: &Path, origin: &Path, overwrite: bool) -> Result<()> {
        self.validate_stored(stored)?;
        move_path(stored, origin, overwrite)
    }

    /// Remove a staged payload that will not be restored (e.g. a metadata file).
    pub fn discard_stored(&self, stored: &Path) -> Result<()> {
        if stored.as_os_str().is_empty() {
            return Ok(());
        }
        self.validate_stored(stored)?;
        match remove_tree(stored) {
            Ok(()) => Ok(()),
            Err(Error::Io { source, .. }) if source.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Delete the whole entry directory after a successful undo.
    pub fn cleanup_entry_dir(&self) -> Result<()> {
        let dir = self.entry_dir();
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
            Err(err) => Err(crate::error::io_err(&dir, err)),
        }
    }

    fn validate_stored(&self, stored: &Path) -> Result<()> {
        let trash = fs::canonicalize(&self.paths.trash_dir)
            .unwrap_or_else(|_| self.paths.trash_dir.clone());
        let candidate = if stored.is_absolute() {
            stored.to_path_buf()
        } else {
            self.paths.trash_dir.join(stored)
        };
        // The stored path may not exist yet after a partial failure, so
        // canonicalize the parent rather than the leaf.
        let resolved = match candidate.canonicalize() {
            Ok(path) => path,
            Err(_) => {
                let parent = candidate.parent().unwrap_or(Path::new("/"));
                let name = candidate.file_name().unwrap_or_default();
                parent
                    .canonicalize()
                    .unwrap_or_else(|_| parent.to_path_buf())
                    .join(name)
            }
        };
        if !resolved.starts_with(&trash) {
            return Err(Error::Refused(format!(
                "journal references {:?}, which is outside rewind's trash directory",
                stored.display().to_string()
            )));
        }
        Ok(())
    }

    /// Never move a directory that contains the trash into the trash.
    fn guard_not_ancestor_of_trash(&self, origin: &Path) -> Result<()> {
        let origin_abs = absolutize(origin)?;
        if let (Ok(origin_real), Ok(trash_real)) = (
            origin_abs.canonicalize(),
            self.paths.trash_dir.canonicalize(),
        ) {
            if trash_real.starts_with(&origin_real) {
                return Err(Error::Refused(format!(
                    "refusing to stage {} because it contains rewind's trash directory",
                    origin.display()
                )));
            }
        }
        Ok(())
    }
}

/// Move a path (file or tree) to a new location, refusing to clobber unless
/// explicitly requested. Shared by undo of `rm`, `mv`, and `git clean`.
pub fn move_path(from: &Path, to: &Path, overwrite: bool) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).path_context(parent)?;
    }
    if to.symlink_metadata().is_ok() {
        if !overwrite {
            return Err(Error::Refused(format!(
                "refusing to overwrite {} (move it away and retry `rewind undo`)",
                to.display()
            )));
        }
        remove_tree(to)?;
    }

    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(err) if is_cross_device(&err) => {
            copy_tree(from, to)?;
            remove_tree(from)?;
            Ok(())
        }
        Err(err) => Err(crate::error::io_err(to, err)),
    }
}

/// Convert a possibly relative path into an absolute one without resolving
/// symlinks (which would change rm/mv semantics).
pub fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let cwd = std::env::current_dir().io_context("reading current directory")?;
        Ok(cwd.join(path))
    }
}

fn is_cross_device(err: &std::io::Error) -> bool {
    err.kind() == ErrorKind::CrossesDevices
        || err.raw_os_error() == Some(18) // EXDEV on Linux/macOS
        || err.raw_os_error() == Some(17) // EXDEV on some BSDs
}

/// Recursively copy preserving modes, symlinks, and timestamps. Returns the
/// number of bytes copied.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<u64> {
    let meta = fs::symlink_metadata(src).path_context(src)?;
    let file_type = meta.file_type();

    if file_type.is_symlink() {
        let target = fs::read_link(src).path_context(src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dst).path_context(dst)?;
        #[cfg(not(unix))]
        {
            // Non-Unix platforms are not a primary target; copy through.
            fs::copy(src, dst).path_context(dst)?;
        }
        return Ok(meta.len());
    }

    if file_type.is_dir() {
        fs::create_dir_all(dst).path_context(dst)?;
        let mut total = 0u64;
        let entries = fs::read_dir(src).path_context(src)?;
        for entry in entries {
            let entry = entry.path_context(src)?;
            let child_src = entry.path();
            let name = entry.file_name();
            total = total.saturating_add(copy_tree(&child_src, &dst.join(name))?);
        }
        copy_permissions(&meta, dst)?;
        copy_times(&meta, dst);
        return Ok(total);
    }

    if file_type.is_file() {
        let copied = fs::copy(src, dst).path_context(src)?;
        copy_permissions(&meta, dst)?;
        copy_times(&meta, dst);
        return Ok(copied);
    }

    // FIFOs, sockets, and device nodes cannot be snapshotted portably. A hard
    // link can preserve them without reading their contents.
    match fs::hard_link(src, dst) {
        Ok(()) => Ok(0),
        Err(err) => Err(Error::Staging(format!(
            "cannot snapshot special file {}: {err}",
            src.display()
        ))),
    }
}

/// Like [`copy_tree`], but hard-links regular files. Only safe when the source
/// will be unlinked rather than rewritten.
fn link_tree(src: &Path, dst: &Path) -> Result<u64> {
    let meta = fs::symlink_metadata(src).path_context(src)?;
    let file_type = meta.file_type();

    if file_type.is_symlink() {
        let target = fs::read_link(src).path_context(src)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, dst).path_context(dst)?;
        #[cfg(not(unix))]
        fs::copy(src, dst).path_context(dst)?;
        return Ok(meta.len());
    }

    if file_type.is_dir() {
        fs::create_dir_all(dst).path_context(dst)?;
        let mut total = 0u64;
        for entry in fs::read_dir(src).path_context(src)? {
            let entry = entry.path_context(src)?;
            let name = entry.file_name();
            total = total.saturating_add(link_tree(&entry.path(), &dst.join(name))?);
        }
        return Ok(total);
    }

    if file_type.is_file() {
        match fs::hard_link(src, dst) {
            Ok(()) => Ok(meta.len()),
            Err(err) if is_cross_device(&err) => {
                let copied = fs::copy(src, dst).path_context(src)?;
                copy_permissions(&meta, dst)?;
                copy_times(&meta, dst);
                Ok(copied)
            }
            Err(err) => Err(crate::error::io_err(src, err)),
        }
    } else {
        match fs::hard_link(src, dst) {
            Ok(()) => Ok(0),
            Err(err) => Err(Error::Staging(format!(
                "cannot snapshot special file {}: {err}",
                src.display()
            ))),
        }
    }
}

/// Recursively delete a path without following symlinks.
pub fn remove_tree(path: &Path) -> Result<()> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(crate::error::io_err(path, err)),
    };
    if meta.file_type().is_dir() {
        for entry in fs::read_dir(path).path_context(path)? {
            let entry = entry.path_context(path)?;
            remove_tree(&entry.path())?;
        }
        fs::remove_dir(path).path_context(path)?;
    } else {
        fs::remove_file(path).path_context(path)?;
    }
    Ok(())
}

/// Total logical size of a tree.
pub fn tree_size(path: &Path) -> Result<u64> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(crate::error::io_err(path, err)),
    };
    if meta.file_type().is_dir() {
        let mut total = 0u64;
        for entry in fs::read_dir(path).path_context(path)? {
            let entry = entry.path_context(path)?;
            total = total.saturating_add(tree_size(&entry.path())?);
        }
        Ok(total)
    } else if meta.file_type().is_symlink() {
        Ok(0)
    } else {
        Ok(meta.len())
    }
}

fn copy_permissions(meta: &fs::Metadata, dst: &Path) -> Result<()> {
    let perms = meta.permissions();
    fs::set_permissions(dst, perms).path_context(dst)
}

fn copy_times(meta: &fs::Metadata, dst: &Path) {
    // Best-effort: timestamps are nice for undo fidelity but not worth failing
    // an otherwise successful restore.
    let atime = filetime::FileTime::from_last_access_time(meta);
    let mtime = filetime::FileTime::from_last_modification_time(meta);
    let _ = filetime::set_file_times(dst, atime, mtime);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_paths(root: &Path) -> Paths {
        Paths {
            data_dir: root.join("data"),
            trash_dir: root.join("data/trash"),
            journal_path: root.join("data/journal.db"),
            lock_path: root.join("data/rewind.lock"),
            config_path: root.join("config.toml"),
        }
    }

    #[test]
    fn move_and_restore_file() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        paths.ensure().unwrap();
        let original = dir.path().join("notes.txt");
        fs::write(&original, b"hello").unwrap();

        let stager = Stager::new(&paths, 1);
        let payload = stager.stage_move(&original).unwrap();
        assert!(!original.exists());
        assert_eq!(fs::read(&payload.stored).unwrap(), b"hello");
        assert_eq!(payload.size, 5);

        stager.restore(&payload.stored, &original, false).unwrap();
        assert_eq!(fs::read(&original).unwrap(), b"hello");
        assert!(!payload.stored.exists());
    }

    #[test]
    fn restore_refuses_to_clobber() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        paths.ensure().unwrap();
        let original = dir.path().join("f");
        fs::write(&original, b"old").unwrap();
        let stager = Stager::new(&paths, 2);
        let payload = stager.stage_move(&original).unwrap();
        fs::write(&original, b"new").unwrap();
        let err = stager
            .restore(&payload.stored, &original, false)
            .unwrap_err();
        assert!(matches!(err, Error::Refused(_)));
        assert_eq!(fs::read(&original).unwrap(), b"new");
    }

    #[test]
    fn snapshot_copies_nested_tree() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        paths.ensure().unwrap();
        let src = dir.path().join("project/src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("main.rs"), b"fn main() {}").unwrap();

        let stager = Stager::new(&paths, 3);
        let payload = stager.snapshot(&src, SnapshotMode::Copy).unwrap();
        assert!(payload.stored.join("main.rs").exists());
        assert!(src.join("main.rs").exists());
        assert_eq!(payload.size, 12);
    }

    #[test]
    fn cross_device_fallback_is_exercised_by_copy_path() {
        // A same-filesystem rename is the fast path; explicitly verify the
        // copy+remove path used for EXDEV and for snapshots.
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        paths.ensure().unwrap();
        let original = dir.path().join("tree");
        fs::create_dir_all(original.join("a/b")).unwrap();
        fs::write(original.join("a/b/c"), b"x").unwrap();

        let dest = dir.path().join("copy");
        let size = copy_tree(&original, &dest).unwrap();
        assert_eq!(size, 1);
        remove_tree(&original).unwrap();
        assert!(!original.exists());
        assert!(dest.join("a/b/c").exists());
    }

    #[test]
    fn refuses_to_stage_a_parent_of_the_trash() {
        let dir = tempdir().unwrap();
        let paths = test_paths(dir.path());
        paths.ensure().unwrap();
        let stager = Stager::new(&paths, 4);
        let err = stager.stage_move(dir.path()).unwrap_err();
        assert!(matches!(err, Error::Refused(_)));
    }

    #[test]
    fn tree_size_counts_files_but_not_symlinks() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("a"), b"1234").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("a", dir.path().join("link")).unwrap();
        assert_eq!(tree_size(dir.path()).unwrap(), 4);
    }
}
