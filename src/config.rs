//! Configuration file handling.
//!
//! The config is optional: in its absence every wrapper is enabled and the
//! trash is kept for 30 days. Setting `wrap.foo = false` makes `rewind exec
//! foo` hand control straight back to the real command without touching the
//! journal, so disabling a wrapper is a true no-op.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::paths::Paths;
use crate::util::parse_bool;

/// Top-level configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    pub wrap: WrapConfig,
    pub trash: TrashConfig,
    pub journal: JournalConfig,
    pub git: GitConfig,
}

/// Which shell commands rewind should intercept.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct WrapConfig {
    pub rm: bool,
    pub mv: bool,
    pub cp: bool,
    pub chmod: bool,
    pub chown: bool,
    /// Master switch for git interception.
    pub git: bool,
    pub git_reset: bool,
    pub git_clean: bool,
    pub git_checkout: bool,
    pub git_restore: bool,
}

impl Default for WrapConfig {
    fn default() -> Self {
        Self {
            rm: true,
            mv: true,
            cp: true,
            chmod: true,
            chown: true,
            git: true,
            git_reset: true,
            git_clean: true,
            git_checkout: true,
            git_restore: true,
        }
    }
}

/// Trash retention policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TrashConfig {
    /// Days to keep staged data. `0` disables automatic expiry.
    pub retention_days: u64,
    /// Soft cap in bytes. `0` means unlimited. `rewind clean` prunes oldest
    /// entries first when the cap is exceeded.
    pub max_bytes: u64,
}

impl Default for TrashConfig {
    fn default() -> Self {
        Self {
            retention_days: 30,
            max_bytes: 0,
        }
    }
}

/// Journal retention.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct JournalConfig {
    /// Days to keep journal rows after their staged data is purged. `0` keeps
    /// rows forever.
    pub keep_days: u64,
}

/// Git-specific behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GitConfig {
    /// Also snapshot untracked files before `git clean` (recommended).
    pub snapshot_untracked: bool,
    /// Keep `refs/rewind/*` snapshots even after the journal entry is purged.
    pub keep_refs: bool,
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            snapshot_untracked: true,
            keep_refs: false,
        }
    }
}

impl Config {
    /// Load the config, returning defaults when the file does not exist.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|err| Error::Config(format!("{}: {err}", path.display()))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(Error::Io {
                context: format!("reading {}", path.display()),
                source: err,
            }),
        }
    }

    /// Write the config atomically, creating parent directories.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).path_context(parent)?;
        }
        let body = toml::to_string_pretty(self)
            .map_err(|err| Error::Config(format!("serializing config: {err}")))?;
        let header = "\
# rewind configuration
# Docs: https://github.com/janghotan/rewind#configuration
# Changes take effect on the next wrapped command.

";
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, format!("{header}{body}")).path_context(&tmp)?;
        std::fs::rename(&tmp, path).path_context(path)?;
        Ok(())
    }

    /// Whether a top-level command is wrapped at all.
    pub fn wraps_command(&self, command: &str) -> bool {
        match command {
            "rm" => self.wrap.rm,
            "mv" => self.wrap.mv,
            "cp" => self.wrap.cp,
            "chmod" => self.wrap.chmod,
            "chown" => self.wrap.chown,
            "git" => self.wrap.git,
            _ => false,
        }
    }

    /// Whether a specific git subcommand should be intercepted.
    pub fn wraps_git_op(&self, op: &str) -> bool {
        if !self.wrap.git {
            return false;
        }
        match op {
            "reset" => self.wrap.git_reset,
            "clean" => self.wrap.git_clean,
            "checkout" => self.wrap.git_checkout,
            "restore" => self.wrap.git_restore,
            _ => false,
        }
    }

    /// Update a dotted key, validating both key and value.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let bool_at = |slot: &mut bool| -> Result<()> {
            *slot = parse_bool(value)?;
            Ok(())
        };
        let u64_at = |slot: &mut u64| -> Result<()> {
            *slot = value
                .parse()
                .map_err(|_| Error::InvalidInput(format!("invalid number {value:?} for {key}")))?;
            Ok(())
        };

        match key {
            "wrap.rm" => bool_at(&mut self.wrap.rm)?,
            "wrap.mv" => bool_at(&mut self.wrap.mv)?,
            "wrap.cp" => bool_at(&mut self.wrap.cp)?,
            "wrap.chmod" => bool_at(&mut self.wrap.chmod)?,
            "wrap.chown" => bool_at(&mut self.wrap.chown)?,
            "wrap.git" => bool_at(&mut self.wrap.git)?,
            "wrap.git_reset" => bool_at(&mut self.wrap.git_reset)?,
            "wrap.git_clean" => bool_at(&mut self.wrap.git_clean)?,
            "wrap.git_checkout" => bool_at(&mut self.wrap.git_checkout)?,
            "wrap.git_restore" => bool_at(&mut self.wrap.git_restore)?,
            "trash.retention_days" => u64_at(&mut self.trash.retention_days)?,
            "trash.max_bytes" => u64_at(&mut self.trash.max_bytes)?,
            "journal.keep_days" => u64_at(&mut self.journal.keep_days)?,
            "git.snapshot_untracked" => bool_at(&mut self.git.snapshot_untracked)?,
            "git.keep_refs" => bool_at(&mut self.git.keep_refs)?,
            other => {
                return Err(Error::InvalidInput(format!(
                    "unknown config key {other:?}; run `rewind config show` to see the keys"
                )));
            }
        }
        Ok(())
    }

    /// All known keys, used by shell completion and `config list`.
    pub fn known_keys() -> &'static [&'static str] {
        &[
            "wrap.rm",
            "wrap.mv",
            "wrap.cp",
            "wrap.chmod",
            "wrap.chown",
            "wrap.git",
            "wrap.git_reset",
            "wrap.git_clean",
            "wrap.git_checkout",
            "wrap.git_restore",
            "trash.retention_days",
            "trash.max_bytes",
            "journal.keep_days",
            "git.snapshot_untracked",
            "git.keep_refs",
        ]
    }
}

/// Load config for a given [`Paths`].
pub fn load(paths: &Paths) -> Result<Config> {
    Config::load(&paths.config_path)
}

/// Write an example config if none exists (used by `rewind config init`).
pub fn init_if_missing(paths: &Paths) -> Result<PathBuf> {
    if paths.config_path.exists() {
        return Ok(paths.config_path.clone());
    }
    Config::default().save(&paths.config_path)?;
    Ok(paths.config_path.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_enable_everything() {
        let config = Config::default();
        assert!(config.wraps_command("rm"));
        assert!(config.wraps_git_op("clean"));
        assert_eq!(config.trash.retention_days, 30);
    }

    #[test]
    fn partial_config_keeps_defaults() {
        let parsed: Config = toml::from_str("[wrap]\nrm = false\n").unwrap();
        assert!(!parsed.wrap.rm);
        assert!(parsed.wrap.mv);
        assert_eq!(parsed.trash.retention_days, 30);
    }

    #[test]
    fn roundtrips_through_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();
        config.trash.retention_days = 7;
        config.wrap.cp = false;
        config.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(loaded, config);
    }

    #[test]
    fn set_validates_keys_and_values() {
        let mut config = Config::default();
        config.set("wrap.rm", "no").unwrap();
        assert!(!config.wrap.rm);
        assert!(config.set("wrap.nope", "true").is_err());
        assert!(config.set("trash.retention_days", "soon").is_err());
    }
}
