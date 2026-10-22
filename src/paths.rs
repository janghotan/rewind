//! XDG-style filesystem layout.
//!
//! Everything rewind writes lives under a single data directory (by default
//! `~/.local/share/rewind`), and configuration lives under
//! `~/.config/rewind/config.toml`. Both can be redirected with environment
//! variables, which is what the integration tests do.

use std::path::{Path, PathBuf};

use crate::error::{Error, IoContext, Result};

/// Resolved locations for all on-disk state.
#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub trash_dir: PathBuf,
    pub journal_path: PathBuf,
    pub lock_path: PathBuf,
    pub config_path: PathBuf,
}

impl Paths {
    /// Discover paths from the environment.
    ///
    /// Overrides (highest priority first):
    /// - `REWIND_DATA_DIR`, `REWIND_JOURNAL`, `REWIND_CONFIG_PATH`
    /// - `XDG_DATA_HOME`, `XDG_CONFIG_HOME`
    /// - `$HOME/.local/share`, `$HOME/.config`
    pub fn discover() -> Result<Self> {
        let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
            Error::Config("HOME is not set; cannot locate rewind's data directory".into())
        })?;

        let data_dir = match std::env::var_os("REWIND_DATA_DIR") {
            Some(dir) if !dir.is_empty() => PathBuf::from(dir),
            _ => match std::env::var_os("XDG_DATA_HOME") {
                Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("rewind"),
                _ => home.join(".local").join("share").join("rewind"),
            },
        };

        let config_path = match std::env::var_os("REWIND_CONFIG_PATH") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => match std::env::var_os("XDG_CONFIG_HOME") {
                Some(dir) if !dir.is_empty() => {
                    PathBuf::from(dir).join("rewind").join("config.toml")
                }
                _ => home.join(".config").join("rewind").join("config.toml"),
            },
        };

        let journal_path = match std::env::var_os("REWIND_JOURNAL") {
            Some(path) if !path.is_empty() => PathBuf::from(path),
            _ => data_dir.join("journal.db"),
        };

        Ok(Self {
            trash_dir: data_dir.join("trash"),
            lock_path: data_dir.join("rewind.lock"),
            data_dir,
            journal_path,
            config_path,
        })
    }

    /// Create the data and trash directories with owner-only permissions.
    pub fn ensure(&self) -> Result<()> {
        create_private_dir(&self.data_dir)?;
        create_private_dir(&self.trash_dir)?;
        Ok(())
    }

    /// Per-entry staging directory (`trash/<id>`).
    pub fn entry_dir(&self, entry_id: i64) -> PathBuf {
        self.trash_dir.join(entry_id.to_string())
    }

    /// Human-readable summary used by `rewind status`.
    pub fn describe(&self) -> String {
        format!(
            "data:   {}\nconfig: {}\njournal: {}",
            self.data_dir.display(),
            self.config_path.display(),
            self.journal_path.display()
        )
    }
}

/// Create a directory (and parents) restricted to the current user on Unix.
pub fn create_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).path_context(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(path, perms).path_context(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_dirs_are_nested_under_trash() {
        let paths = Paths {
            data_dir: PathBuf::from("/d"),
            trash_dir: PathBuf::from("/d/trash"),
            journal_path: PathBuf::from("/d/journal.db"),
            lock_path: PathBuf::from("/d/rewind.lock"),
            config_path: PathBuf::from("/c/config.toml"),
        };
        assert_eq!(paths.entry_dir(7), PathBuf::from("/d/trash/7"));
        assert!(paths.describe().contains("/d/journal.db"));
    }
}
