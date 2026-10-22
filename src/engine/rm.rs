//! `rm` interception.
//!
//! Deletion is implemented as an atomic rename into the trash, which makes
//! undo O(1) and byte-exact. Argument handling mirrors GNU coreutils closely:
//! the same success/failure exit codes, the same quiet-on-success behavior,
//! `-v` output, `-i`/`-I` prompting, and `--preserve-root` protection.
//!
//! When staging is impossible (for example a read-only parent), rewind falls
//! back to actually deleting the operand so that `rm` never becomes less
//! capable than the real thing. Such operations are recorded as non-undoable
//! rather than pretending they can be reversed.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde_json::json;

use crate::error::{Error, Result};
use crate::journal::{EntryStatus, Journal, NewStaged, StagedKind};
use crate::paths::Paths;
use crate::staging::{Stager, absolutize, remove_tree};

use super::{
    coreutils_message, ends_with_dot_component, is_filesystem_root, run_passthrough,
    stdin_is_terminal,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interactive {
    Never,
    Always,
    Once,
}

#[derive(Debug)]
struct Options {
    force: bool,
    recursive: bool,
    dir: bool,
    verbose: bool,
    interactive: Interactive,
    one_file_system: bool,
    no_preserve_root: bool,
    paths: Vec<String>,
}

enum Parsed {
    Run(Box<Options>),
    /// Hand the whole invocation to the real `rm`, preserving behavior for
    /// options rewind does not model (`--help`, `--one-file-system`, unknown
    /// flags, and support for non-UTF-8 arguments at the CLI layer).
    Passthrough,
}

pub fn execute(journal: &mut Journal, paths: &Paths, argv: &[String]) -> Result<i32> {
    let options = match parse(argv) {
        Parsed::Passthrough => return run_passthrough("rm", argv),
        Parsed::Run(options) => options,
    };

    if options.paths.is_empty() {
        eprintln!("rm: missing operand");
        eprintln!("Try 'rm --help' for more information.");
        return Ok(1);
    }

    // `rm -r link/` operates on the link target, not the link. Rather than
    // approximate that, let the real rm do it (without undo for this call).
    if options.recursive && options.paths.iter().any(|raw| follows_trailing_slash(raw)) {
        return run_passthrough("rm", argv);
    }

    let cwd = env::current_dir().map_err(|err| Error::Io {
        context: "reading current directory".into(),
        source: err,
    })?;
    let entry_id = journal.begin_entry("rm", argv, &cwd)?;
    let stager = Stager::new(paths, entry_id);

    if options.interactive == Interactive::Once
        && (options.paths.len() > 3 || options.recursive)
        && !confirm(&format!(
            "rm: remove {} argument{}? ",
            options.paths.len(),
            if options.paths.len() == 1 { "" } else { "s" }
        ))
    {
        journal.delete_entry(entry_id)?;
        return Ok(0);
    }

    let mut exit_code = 0;
    let mut staged_count = 0usize;
    let mut unsaved_count = 0usize;

    for raw in &options.paths {
        let absolute = match absolutize(Path::new(raw)) {
            Ok(path) => path,
            Err(err) => {
                eprintln!("rm: cannot remove '{raw}': {err}");
                exit_code = 1;
                continue;
            }
        };

        match process_operand(&options, raw, &absolute, &stager, journal, entry_id) {
            Ok(OperandResult::Staged) => staged_count += 1,
            Ok(OperandResult::DeletedDirectly) => {
                unsaved_count += 1;
            }
            Ok(OperandResult::Skipped) => {}
            Err(OperandError::Message(message)) => {
                eprintln!("rm: cannot remove '{raw}': {message}");
                exit_code = 1;
            }
            Err(OperandError::Raw(message)) => {
                eprintln!("{message}");
                exit_code = 1;
            }
        }
    }

    let undoable = staged_count > 0;
    if staged_count == 0 && unsaved_count == 0 {
        // Pure no-op (`rm -f missing`) or an uninteresting failure: do not
        // clutter the undo stack with entries that cannot do anything.
        journal.delete_entry(entry_id)?;
        stager.cleanup_entry_dir()?;
    } else {
        let status = if exit_code == 0 || undoable {
            EntryStatus::Done
        } else {
            EntryStatus::Failed
        };
        journal.finish_entry(
            entry_id,
            status,
            Some(exit_code),
            undoable,
            Some(&json!({
                "staged": staged_count,
                "unsaved": unsaved_count,
            })),
        )?;
    }

    Ok(exit_code)
}

enum OperandResult {
    Staged,
    DeletedDirectly,
    Skipped,
}

enum OperandError {
    /// `rm: cannot remove '<operand>': <message>`
    Message(String),
    /// A fully formatted message.
    Raw(String),
}

fn process_operand(
    options: &Options,
    raw: &str,
    absolute: &Path,
    stager: &Stager<'_>,
    journal: &mut Journal,
    entry_id: i64,
) -> std::result::Result<OperandResult, OperandError> {
    let meta = match fs::symlink_metadata(absolute) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if options.force {
                return Ok(OperandResult::Skipped);
            }
            return Err(OperandError::Message(coreutils_message(&err)));
        }
        Err(err) => return Err(OperandError::Message(coreutils_message(&err))),
    };

    let is_dir = meta.file_type().is_dir();
    if is_dir && ends_with_dot_component(raw) {
        return Err(OperandError::Raw(format!(
            "rm: refusing to remove '.' or '..' directory: skipping '{raw}'"
        )));
    }

    if is_dir && options.recursive && is_filesystem_root(absolute) && !options.no_preserve_root {
        return Err(OperandError::Raw(
            "rm: it is dangerous to operate recursively on '/'\n\
             rm: use --no-preserve-root to override this failsafe"
                .into(),
        ));
    }

    if is_dir && !options.recursive {
        if !options.dir {
            return Err(OperandError::Message("Is a directory".into()));
        }
        match fs::read_dir(absolute) {
            Ok(mut entries) => {
                if entries.next().is_some() {
                    return Err(OperandError::Message("Directory not empty".into()));
                }
            }
            Err(err) => return Err(OperandError::Message(coreutils_message(&err))),
        }
    }

    if !options.force && options.interactive == Interactive::Always {
        let what = describe(&meta);
        if !confirm(&format!("rm: remove {what} '{raw}'? ")) {
            return Ok(OperandResult::Skipped);
        }
    } else if !options.force && options.interactive == Interactive::Never && stdin_is_terminal() {
        // GNU rm asks before deleting write-protected regular files when
        // interactive confirmation has not already been requested.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.file_type().is_file()
                && meta.permissions().mode() & 0o222 == 0
                && !confirm(&format!(
                    "rm: remove write-protected regular file '{raw}'? "
                ))
            {
                return Ok(OperandResult::Skipped);
            }
        }
    }

    match stager.stage_move(absolute) {
        Ok(payload) => {
            let item = NewStaged::new(
                absolute.display().to_string(),
                payload.stored.display().to_string(),
                StagedKind::Move,
            )
            .with_size(payload.size);
            if let Err(err) = journal.add_staged(entry_id, &item) {
                // Never lose the user's data because the journal hiccuped:
                // put it back, then report the failure as a normal rm error.
                if stager.restore(&payload.stored, absolute, true).is_ok() {
                    return Err(OperandError::Message(err.to_string()));
                }
                return Err(OperandError::Message(format!(
                    "{err}; data was kept at {}",
                    payload.stored.display()
                )));
            }
            if options.verbose {
                println!("removed '{raw}'");
            }
            Ok(OperandResult::Staged)
        }
        Err(Error::Refused(message)) => Err(OperandError::Message(message)),
        Err(stage_err) => {
            // Preserve rm's contract: if we cannot stage it, delete it.
            match remove_tree(absolute) {
                Ok(()) => {
                    if options.verbose {
                        println!("removed '{raw}'");
                    }
                    let _ = stage_err;
                    Ok(OperandResult::DeletedDirectly)
                }
                Err(_) => Err(OperandError::Message(match stage_err {
                    Error::Io { source, .. } => coreutils_message(&source),
                    other => other.to_string(),
                })),
            }
        }
    }
}

fn describe(meta: &fs::Metadata) -> &'static str {
    let ft = meta.file_type();
    if ft.is_dir() {
        "directory"
    } else if ft.is_symlink() {
        "symbolic link"
    } else if ft.is_file() && meta.len() == 0 {
        "regular empty file"
    } else {
        "regular file"
    }
}

fn confirm(prompt: &str) -> bool {
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.chars().next(), Some('y' | 'Y'))
}

fn follows_trailing_slash(raw: &str) -> bool {
    if !raw.ends_with('/') || raw == "/" {
        return false;
    }
    fs::symlink_metadata(raw)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn parse(argv: &[String]) -> Parsed {
    let mut options = Options {
        force: false,
        recursive: false,
        dir: false,
        verbose: false,
        interactive: Interactive::Never,
        one_file_system: false,
        no_preserve_root: false,
        paths: Vec::new(),
    };
    let mut only_paths = false;

    for arg in argv {
        if only_paths {
            options.paths.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_paths = true;
            continue;
        }
        if arg == "-" || !arg.starts_with('-') {
            options.paths.push(arg.clone());
            continue;
        }

        if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            match name {
                "force" => {
                    options.force = true;
                    options.interactive = Interactive::Never;
                }
                "recursive" => options.recursive = true,
                "dir" => options.dir = true,
                "verbose" => options.verbose = true,
                "interactive" => {
                    options.interactive = match value.unwrap_or("always") {
                        "never" | "no" | "none" => Interactive::Never,
                        "once" => Interactive::Once,
                        "always" | "yes" => Interactive::Always,
                        _ => return Parsed::Passthrough,
                    };
                    if options.interactive != Interactive::Never {
                        options.force = false;
                    }
                }
                "one-file-system" => options.one_file_system = true,
                "no-preserve-root" => options.no_preserve_root = true,
                "preserve-root" => options.no_preserve_root = false,
                "help" | "version" => return Parsed::Passthrough,
                _ => return Parsed::Passthrough,
            }
            continue;
        }

        // Short option cluster, e.g. -rf or -rfv.
        for flag in arg.chars().skip(1) {
            match flag {
                'f' => {
                    options.force = true;
                    options.interactive = Interactive::Never;
                }
                'r' | 'R' => options.recursive = true,
                'd' => options.dir = true,
                'v' => options.verbose = true,
                'i' => {
                    options.interactive = Interactive::Always;
                    options.force = false;
                }
                'I' => {
                    options.interactive = Interactive::Once;
                    options.force = false;
                }
                _ => return Parsed::Passthrough,
            }
        }
    }

    // --one-file-system requires per-file traversal inside mounted
    // subdirectories; let the real rm handle it rather than risk moving a
    // mount point wholesale.
    if options.one_file_system {
        return Parsed::Passthrough;
    }

    Parsed::Run(Box::new(options))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn parses_combined_flags() {
        let Parsed::Run(options) = parse(&args(&["-rfv", "build"])) else {
            panic!("expected run");
        };
        assert!(options.force && options.recursive && options.verbose);
        assert_eq!(options.paths, vec!["build"]);
    }

    #[test]
    fn last_interactive_flag_wins() {
        let Parsed::Run(options) = parse(&args(&["-i", "-f", "x"])) else {
            panic!("expected run");
        };
        assert!(options.force);
        assert_eq!(options.interactive, Interactive::Never);

        let Parsed::Run(options) = parse(&args(&["-f", "-i", "x"])) else {
            panic!("expected run");
        };
        assert!(!options.force);
        assert_eq!(options.interactive, Interactive::Always);
    }

    #[test]
    fn unknown_options_passthrough() {
        assert!(matches!(
            parse(&args(&["--frobnicate", "x"])),
            Parsed::Passthrough
        ));
        assert!(matches!(parse(&args(&["-Z", "x"])), Parsed::Passthrough));
        assert!(matches!(parse(&args(&["--help"])), Parsed::Passthrough));
    }

    #[test]
    fn dash_is_a_path() {
        let Parsed::Run(options) = parse(&args(&["-f", "--", "-"])) else {
            panic!("expected run");
        };
        assert_eq!(options.paths, vec!["-"]);
    }

    #[test]
    fn one_file_system_passthrough() {
        assert!(matches!(
            parse(&args(&["--one-file-system", "-r", "x"])),
            Parsed::Passthrough
        ));
    }
}
