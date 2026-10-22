//! Domain error type shared by all library modules.
//!
//! Binary-level code (see `main.rs`) converts these into `anyhow` reports; the
//! library itself never panics and never unwraps on fallible operations.

use std::path::Path;

/// Everything that can go wrong inside rewind.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An I/O operation failed, with the path that caused it.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    /// Configuration was missing, malformed, or could not be written.
    #[error("config error: {0}")]
    Config(String),

    /// The journal database could not be opened or queried.
    #[error("journal error: {0}")]
    Journal(String),

    /// A staging (trash) operation failed.
    #[error("staging error: {0}")]
    Staging(String),

    /// A git snapshot or restore failed.
    #[error("git error: {0}")]
    Git(String),

    /// The user passed something rewind cannot interpret.
    #[error("{0}")]
    InvalidInput(String),

    /// rewind deliberately declined to perform a destructive step.
    #[error("{0}")]
    Refused(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Attach a path to an [`std::io::Error`].
pub trait IoContext<T> {
    fn io_context(self, context: impl Into<String>) -> Result<T>;
    fn path_context(self, path: &Path) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn io_context(self, context: impl Into<String>) -> Result<T> {
        self.map_err(|source| Error::Io {
            context: context.into(),
            source,
        })
    }

    fn path_context(self, path: &Path) -> Result<T> {
        self.map_err(|source| Error::Io {
            context: format!("{}", path.display()),
            source,
        })
    }
}

/// Convenience for turning a path into a lossy display string.
pub fn display(path: &Path) -> String {
    path.display().to_string()
}

/// Build an [`Error::Io`] for a path with extra context.
pub fn io_err(path: impl AsRef<Path>, source: std::io::Error) -> Error {
    Error::Io {
        context: path.as_ref().display().to_string(),
        source,
    }
}

/// Build an [`Error::Io`] for a path-less syscall (spawn, pipe, ...).
pub fn plain_io(source: std::io::Error) -> Error {
    Error::Io {
        context: "I/O operation failed".into(),
        source,
    }
}

/// Convert a [`PathBuf`] into a UTF-8 string, failing loudly rather than
/// silently mangling user paths.
pub fn path_to_string(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::InvalidInput(format!("path is not valid UTF-8: {}", path.display())))
}
