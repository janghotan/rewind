//! rewind — universal undo for destructive shell commands.
//!
//! The library is split into small modules so that each piece can be tested in
//! isolation:
//!
//! * [`paths`] resolves the XDG layout.
//! * [`config`] parses the TOML configuration.
//! * [`journal`] persists entries and staged payloads in SQLite.
//! * [`staging`] moves/copies data into and out of the trash.
//! * [`engine`] contains one interceptor per wrapped command.
//! * [`undo`] replays a journal entry backwards.
//! * [`commands`] implements `log`, `status`, and `clean`.

pub mod commands;
pub mod config;
pub mod engine;
pub mod error;
pub mod journal;
pub mod paths;
pub mod staging;
pub mod undo;
pub mod util;

pub use error::{Error, Result};
