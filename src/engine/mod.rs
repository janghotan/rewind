//! Interceptors for each wrapped command.
//!
//! Commands that are not destructive on their own (`cp`, `mv`, `chmod`,
//! `chown`, and git) take a snapshot first and then hand execution to the real
//! binary, inheriting stdio. That keeps their output, prompts, and exit codes
//! byte-for-byte native. `rm` is implemented directly because the efficient
//! undo strategy is a rename into the trash, which the real `rm` cannot be
//! asked to do.

pub mod chmod;
pub mod chown;
pub mod cp;
pub mod git;
pub mod metadata;
pub mod mv;
pub mod rm;
pub mod transfer;

use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::journal::Journal;
use crate::paths::Paths;

/// Run an intercepted command and return the process exit code.
pub fn dispatch(
    journal: &mut Journal,
    paths: &Paths,
    config: &Config,
    command: &str,
    argv: &[String],
) -> Result<i32> {
    match command {
        "rm" => rm::execute(journal, paths, argv),
        "mv" => mv::execute(journal, paths, argv),
        "cp" => cp::execute(journal, paths, argv),
        "chmod" => chmod::execute(journal, paths, argv),
        "chown" => chown::execute(journal, paths, argv),
        "git" => git::execute(journal, paths, config, argv),
        other => Err(Error::InvalidInput(format!(
            "rewind does not know how to intercept {other:?}"
        ))),
    }
}

/// Execute the real command with inherited stdio and return its exit code.
pub fn run_passthrough(command: &str, argv: &[String]) -> Result<i32> {
    let status = Command::new(command)
        .args(argv)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|err| Error::Io {
            context: format!("running {command}"),
            source: err,
        })?;
    Ok(exit_code(status))
}

/// Convert a process status into a shell exit code.
pub fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

/// Format an I/O error the way coreutils does in the C locale.
pub fn coreutils_message(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::NotFound => "No such file or directory".into(),
        std::io::ErrorKind::PermissionDenied => "Permission denied".into(),
        std::io::ErrorKind::IsADirectory => "Is a directory".into(),
        std::io::ErrorKind::DirectoryNotEmpty => "Directory not empty".into(),
        std::io::ErrorKind::AlreadyExists => "File exists".into(),
        std::io::ErrorKind::InvalidInput => "Invalid argument".into(),
        _ => err.to_string(),
    }
}

/// Whether stdout is a terminal (used for command-specific niceties).
pub fn stdout_is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Whether stdin is a terminal.
pub fn stdin_is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// Whether stderr is a terminal.
pub fn stderr_is_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}

/// Warn the user without polluting piped/scripted output.
///
/// rewind must be invisible to scripts; warnings about failed snapshots are
/// still worth showing to a human at a terminal, and are always recorded in
/// the journal entry's metadata.
pub fn warn(message: &str) {
    if stderr_is_terminal() {
        eprintln!("rewind: {message}");
    }
}

/// Best-effort check that a path names the filesystem root.
pub fn is_filesystem_root(path: &Path) -> bool {
    let mut components = path.components();
    matches!(
        (components.next(), components.next()),
        (Some(std::path::Component::RootDir), None)
    )
}

/// Lexically check the final component of a user-supplied path for `.` or `..`.
pub fn ends_with_dot_component(raw: &str) -> bool {
    let trimmed = raw.trim_end_matches('/');
    trimmed == "." || trimmed == ".." || trimmed.ends_with("/.") || trimmed.ends_with("/..")
}
