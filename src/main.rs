//! CLI entry point.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use clap_complete::Shell as CompletionShell;

use rewind::commands::{self, CleanOptions};
use rewind::config::{self, Config};
use rewind::engine;
use rewind::journal::Journal;
use rewind::paths::Paths;
use rewind::undo;

#[derive(Parser)]
#[command(
    name = "rewind",
    version,
    about = "Undo for destructive shell commands",
    long_about = "rewind intercepts rm, mv, cp, chmod, chown, and destructive git \
commands, stages their pre-action state, and lets you reverse them.\n\n\
Shell integration:\n  bash   echo 'eval \"$(rewind init bash)\"' >> ~/.bashrc\n  zsh    echo 'eval \"$(rewind init zsh)\"' >> ~/.zshrc\n  fish   echo 'rewind init fish | source' >> ~/.config/fish/config.fish\n\n\
Then run `rewind undo`, `rewind log`, and `rewind status`.",
    after_help = "Docs: https://github.com/janghotan/rewind"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Execute a wrapped command (called by the shell hooks)
    #[command(hide = true)]
    Exec {
        /// Command to run: rm, mv, cp, chmod, chown, or git
        command: String,
        /// Arguments passed through unchanged
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },

    /// Reverse the last command, or the last N commands
    Undo {
        /// Number of journal entries to undo (newest first)
        #[arg(default_value_t = 1)]
        n: usize,
        /// Show what would be undone without changing anything
        #[arg(long)]
        dry_run: bool,
    },

    /// Show undo history
    Log {
        /// Maximum number of entries to show
        #[arg(long, short = 'n', default_value_t = 30)]
        limit: usize,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Inspect one journal entry without changing anything
    Inspect {
        /// Journal entry ID (see `rewind log`)
        id: i64,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Purge staged files older than the retention period
    Clean {
        /// Override the configured retention, e.g. 7d, 12h, 30m
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,
        /// Purge everything, regardless of age
        #[arg(long)]
        all: bool,
        /// Show what would be purged without deleting anything
        #[arg(long)]
        dry_run: bool,
    },

    /// Show data locations, undo stack, and configuration summary
    Status {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },

    /// Print the shell integration script
    Init {
        /// Shell to emit a hook for
        shell: ShellName,
    },

    /// Print shell completions
    Completions {
        /// Shell to emit completions for
        shell: CompletionShell,
    },

    /// Inspect or change configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ShellName {
    Bash,
    Zsh,
    Fish,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the config file path
    Path,
    /// Print the effective configuration
    Show,
    /// Print one value (for example wrap.rm)
    Get { key: String },
    /// Change one value and save the config
    Set { key: String, value: String },
    /// Create the config with defaults if it does not exist
    Init,
    /// Open the config in $VISUAL/$EDITOR
    Edit,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(err) => {
            eprintln!("rewind: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<i32> {
    let cli = Cli::parse();
    let paths = Paths::discover()?;
    let config = config::load(&paths)?;

    match cli.command {
        Commands::Exec { command, args } => run_exec(&paths, &config, &command, args),
        Commands::Undo { n, dry_run } => {
            paths.ensure()?;
            let mut journal = Journal::open(&paths.journal_path)?;
            let report = undo::run(&mut journal, &paths, n, dry_run)?;
            if report.failures > 0 { Ok(1) } else { Ok(0) }
        }
        Commands::Log { limit, json } => {
            let journal = open_readonly(&paths)?;
            commands::log(&journal, limit, json)?;
            Ok(0)
        }
        Commands::Inspect { id, json } => {
            let journal = Journal::open_query_only(&paths.journal_path)?;
            commands::inspect(&journal, &paths, id, json)?;
            Ok(0)
        }
        Commands::Clean {
            older_than,
            all,
            dry_run,
        } => {
            paths.ensure()?;
            let mut journal = Journal::open(&paths.journal_path)?;
            commands::clean(
                &mut journal,
                &paths,
                &config,
                &CleanOptions {
                    older_than,
                    all,
                    dry_run,
                },
            )?;
            Ok(0)
        }
        Commands::Status { json } => {
            let journal = open_readonly(&paths)?;
            commands::status(&journal, &paths, &config, json)?;
            Ok(0)
        }
        Commands::Init { shell } => {
            let script = match shell {
                ShellName::Bash => include_str!("../shell/bash.sh"),
                ShellName::Zsh => include_str!("../shell/zsh.sh"),
                ShellName::Fish => include_str!("../shell/fish.fish"),
            };
            print!("{script}");
            std::io::stdout().flush().ok();
            Ok(0)
        }
        Commands::Completions { shell } => {
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "rewind", &mut std::io::stdout());
            Ok(0)
        }
        Commands::Config { command } => run_config(&paths, config, command),
    }
}

fn open_readonly(paths: &Paths) -> Result<Journal> {
    paths.ensure()?;
    Ok(Journal::open(&paths.journal_path)?)
}

fn run_exec(paths: &Paths, config: &Config, command: &str, mut args: Vec<OsString>) -> Result<i32> {
    if args.first().is_some_and(|arg| arg == "--") {
        args.remove(0);
    }
    let base = Path::new(command)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(command)
        .to_string();

    let disabled = matches!(
        std::env::var("REWIND_DISABLE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    );
    if disabled || !config.wraps_command(&base) {
        return passthrough_os(command, &args);
    }

    // Non-UTF-8 arguments are rare and cannot be journaled faithfully. Rather
    // than mangle them, hand the whole invocation to the real command; the
    // caller still gets native behavior, just without undo for that call.
    let mut string_args = Vec::with_capacity(args.len());
    for arg in &args {
        match arg.clone().into_string() {
            Ok(value) => string_args.push(value),
            Err(_) => {
                engine::warn("non-UTF-8 argument; running without undo for this call");
                return passthrough_os(command, &args);
            }
        }
    }

    paths.ensure()?;
    let mut journal = Journal::open(&paths.journal_path)?;
    let code = engine::dispatch(&mut journal, paths, config, &base, &string_args)?;
    Ok(code)
}

fn passthrough_os(command: &str, args: &[OsString]) -> Result<i32> {
    let status = match Command::new(command)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
    {
        Ok(status) => status,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("{command}: command not found");
            return Ok(127);
        }
        Err(err) => {
            return Err(anyhow::Error::new(err).context(format!("running {command}")));
        }
    };
    Ok(engine::exit_code(status))
}

fn run_config(paths: &Paths, mut config: Config, command: ConfigCommand) -> Result<i32> {
    match command {
        ConfigCommand::Path => println!("{}", paths.config_path.display()),
        ConfigCommand::Show => {
            println!("# {}", paths.config_path.display());
            print!("{}", toml::to_string_pretty(&config)?);
        }
        ConfigCommand::Get { key } => {
            let document: toml::Value = toml::from_str(&toml::to_string(&config)?)?;
            let mut value = &document;
            for part in key.split('.') {
                value = value
                    .get(part)
                    .with_context(|| format!("unknown config key {key:?}"))?;
            }
            println!("{}", scalar(value));
        }
        ConfigCommand::Set { key, value } => {
            config.set(&key, &value)?;
            config.save(&paths.config_path)?;
            println!("{key} = {value}");
            println!("# saved to {}", paths.config_path.display());
        }
        ConfigCommand::Init => {
            let path = config::init_if_missing(paths)?;
            println!("{}", path.display());
        }
        ConfigCommand::Edit => {
            let path = config::init_if_missing(paths)?;
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .unwrap_or_else(|_| "vi".to_string());
            let status = Command::new(&editor)
                .arg(&path)
                .status()
                .with_context(|| format!("launching editor {editor:?}"))?;
            if !status.success() {
                anyhow::bail!("editor exited with status {status}");
            }
        }
    }
    Ok(0)
}

fn scalar(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}
