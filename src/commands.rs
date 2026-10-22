//! User-facing commands: `log`, `status`, and `clean`.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::journal::{Journal, Stats};
use crate::paths::Paths;
use crate::staging::{remove_tree, tree_size};
use crate::util::{ellipsize, format_timestamp, human_size, now_unix, parse_duration, time_ago};

/// Show journal history.
pub fn log(journal: &Journal, limit: usize, as_json: bool) -> Result<()> {
    let entries = journal.list(limit, true)?;
    if as_json {
        let mut rows = Vec::new();
        for entry in &entries {
            let (items, size) = journal.staged_summary(entry.id)?;
            rows.push(json!({
                "id": entry.id,
                "created_at": entry.created_at,
                "when": format_timestamp(entry.created_at),
                "command": entry.command_line(),
                "cwd": entry.cwd,
                "status": entry.status.as_str(),
                "undoable": entry.is_undoable(),
                "exit_code": entry.exit_code,
                "items": items,
                "bytes": size,
            }));
        }
        let out = serde_json::to_string_pretty(&rows)
            .map_err(|err| Error::Journal(format!("serializing log: {err}")))?;
        println!("{out}");
        return Ok(());
    }

    if entries.is_empty() {
        println!("rewind: no history yet");
        return Ok(());
    }

    println!(
        "{:>5}  {:<19}  {:<11}  {:>9}  COMMAND",
        "ID", "WHEN", "STATUS", "SIZE"
    );
    for entry in &entries {
        let (_items, size) = journal.staged_summary(entry.id)?;
        let status = if entry.is_undoable() {
            "undoable".to_string()
        } else if entry.status == crate::journal::EntryStatus::Interrupted {
            "interrupted".to_string()
        } else {
            entry.status.as_str().to_string()
        };
        let size_text = if size > 0 {
            human_size(size)
        } else if entry.command == "git_reset" {
            "snapshot".to_string()
        } else {
            "-".to_string()
        };
        println!(
            "{:>5}  {:<19}  {:<11}  {:>9}  {}",
            entry.id,
            format_timestamp(entry.created_at),
            status,
            size_text,
            ellipsize(&entry.command_line(), 60),
        );
    }
    Ok(())
}

/// Print a health/status summary.
pub fn status(journal: &Journal, paths: &Paths, config: &Config, as_json: bool) -> Result<()> {
    let stats = journal.stats()?;
    let journal_bytes = fs::metadata(&paths.journal_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let trash_bytes = tree_size(&paths.trash_dir)?;
    let wrappers = enabled_wrappers(config);
    let hook = installed_hook();

    if as_json {
        let out = json!({
            "version": env!("CARGO_PKG_VERSION"),
            "data_dir": paths.data_dir.display().to_string(),
            "journal": paths.journal_path.display().to_string(),
            "journal_bytes": journal_bytes,
            "config": paths.config_path.display().to_string(),
            "trash_bytes": trash_bytes,
            "entries": stats.total_entries,
            "undoable": stats.undoable_entries,
            "undone": stats.undone_entries,
            "expired": stats.expired_entries,
            "oldest_undoable": stats.oldest_undoable,
            "retention_days": config.trash.retention_days,
            "max_bytes": config.trash.max_bytes,
            "wrappers": wrappers,
            "shell_hook": hook,
        });
        let text = serde_json::to_string_pretty(&out)
            .map_err(|err| Error::Journal(format!("serializing status: {err}")))?;
        println!("{text}");
        return Ok(());
    }

    println!("rewind {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("  data      {}", paths.data_dir.display());
    println!("  config    {}", paths.config_path.display());
    println!(
        "  journal   {} ({})",
        paths.journal_path.display(),
        human_size(journal_bytes)
    );
    println!(
        "  trash     {} ({})",
        paths.trash_dir.display(),
        human_size(trash_bytes)
    );
    println!();
    print_stats(&stats);
    println!();
    println!(
        "  retention {}",
        if config.trash.retention_days == 0 {
            "disabled (trash kept until `rewind clean --older-than`)".to_string()
        } else {
            format!(
                "{} days{}",
                config.trash.retention_days,
                if config.trash.max_bytes > 0 {
                    format!(", cap {}", human_size(config.trash.max_bytes))
                } else {
                    String::new()
                }
            )
        }
    );
    println!(
        "  wrappers  {}",
        if wrappers.is_empty() {
            "none enabled".to_string()
        } else {
            wrappers.join(" ")
        }
    );
    if config.wrap.git {
        let mut git_ops = Vec::new();
        for (name, enabled) in [
            ("reset", config.wrap.git_reset),
            ("clean", config.wrap.git_clean),
            ("checkout", config.wrap.git_checkout),
            ("restore", config.wrap.git_restore),
        ] {
            if enabled {
                git_ops.push(name);
            }
        }
        println!("  git ops   {}", git_ops.join(" "));
    }
    println!(
        "  shell     {}",
        match hook {
            Some(shell) => format!("{shell} hook found"),
            None => "no hook found (see `rewind init --help`)".to_string(),
        }
    );
    Ok(())
}

fn print_stats(stats: &Stats) {
    println!(
        "  undo      {} command{} available",
        stats.undoable_entries,
        if stats.undoable_entries == 1 { "" } else { "s" }
    );
    if let Some(oldest) = stats.oldest_undoable {
        println!("            oldest {}", time_ago(oldest));
    }
    println!(
        "  history   {} total, {} undone, {} expired",
        stats.total_entries, stats.undone_entries, stats.expired_entries
    );
    println!("  staged    {}", human_size(stats.staged_bytes));
}

fn enabled_wrappers(config: &Config) -> Vec<String> {
    let mut out = Vec::new();
    for (name, enabled) in [
        ("rm", config.wrap.rm),
        ("mv", config.wrap.mv),
        ("cp", config.wrap.cp),
        ("chmod", config.wrap.chmod),
        ("chown", config.wrap.chown),
        ("git", config.wrap.git),
    ] {
        if enabled {
            out.push(name.to_string());
        }
    }
    out
}

fn installed_hook() -> Option<&'static str> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let candidates = [
        ("bash", home.join(".bashrc")),
        ("zsh", home.join(".zshrc")),
        ("fish", home.join(".config/fish/config.fish")),
    ];
    for (shell, path) in candidates {
        if let Ok(text) = fs::read_to_string(path) {
            if text.contains("rewind init") || text.contains("rewind/shell/") {
                return Some(shell);
            }
        }
    }
    None
}

/// Options for `rewind clean`.
#[derive(Debug, Default)]
pub struct CleanOptions {
    pub older_than: Option<String>,
    pub all: bool,
    pub dry_run: bool,
}

/// Outcome of a cleaning pass.
#[derive(Debug, Default)]
pub struct CleanReport {
    pub entries: usize,
    pub bytes: u64,
    pub history_pruned: usize,
}

/// Purge staged trash according to age and the configured size cap.
pub fn clean(
    journal: &mut Journal,
    paths: &Paths,
    config: &Config,
    options: &CleanOptions,
) -> Result<CleanReport> {
    let cutoff = if options.all {
        now_unix() + 1
    } else if let Some(duration) = &options.older_than {
        let seconds = parse_duration(duration)? as i64;
        now_unix() - seconds
    } else if config.trash.retention_days > 0 {
        now_unix() - (config.trash.retention_days as i64) * 86_400
    } else {
        return Err(Error::InvalidInput(
            "retention is disabled (trash.retention_days = 0); pass --older-than <duration> or --all"
                .into(),
        ));
    };

    let mut report = CleanReport::default();

    if options.dry_run {
        let ids = journal.expirable_ids(cutoff)?;
        report.entries = ids.len();
        for id in ids {
            report.bytes = report
                .bytes
                .saturating_add(tree_size(&paths.entry_dir(id)).unwrap_or(0));
        }
        // Preview size-cap pruning.
        if config.trash.max_bytes > 0 {
            let sizes = journal.undoable_sizes()?;
            let mut total: u64 = sizes.iter().map(|(_, _, size)| *size).sum();
            let mut extra = 0usize;
            for (_, _, size) in &sizes {
                if total <= config.trash.max_bytes {
                    break;
                }
                total = total.saturating_sub(*size);
                extra += 1;
            }
            if extra > 0 {
                println!(
                    "rewind: would also purge {extra} older entries to respect the {} cap",
                    human_size(config.trash.max_bytes)
                );
            }
        }
        println!(
            "rewind: would purge {} entr{} ({})",
            report.entries,
            if report.entries == 1 { "y" } else { "ies" },
            human_size(report.bytes)
        );
        return Ok(report);
    }

    let ids = journal.expire_staged_before(cutoff)?;
    for id in &ids {
        let dir = paths.entry_dir(*id);
        report.bytes = report.bytes.saturating_add(tree_size(&dir).unwrap_or(0));
        remove_tree(&dir)?;
        report.entries += 1;
    }

    // Enforce the optional size cap, oldest entries first.
    if config.trash.max_bytes > 0 {
        let sizes = journal.undoable_sizes()?;
        let mut total: u64 = sizes.iter().map(|(_, _, size)| *size).sum();
        for (id, _, size) in sizes {
            if total <= config.trash.max_bytes {
                break;
            }
            journal.expire_entry(id)?;
            let dir = paths.entry_dir(id);
            report.bytes = report.bytes.saturating_add(tree_size(&dir).unwrap_or(0));
            remove_tree(&dir)?;
            total = total.saturating_sub(size);
            report.entries += 1;
        }
    }

    if config.journal.keep_days > 0 {
        let history_cutoff = now_unix() - (config.journal.keep_days as i64) * 86_400;
        report.history_pruned = journal.prune_finished_before(history_cutoff)?;
    }

    if report.entries > 0 {
        let _ = journal.vacuum();
        println!(
            "rewind: purged {} entr{} ({})",
            report.entries,
            if report.entries == 1 { "y" } else { "ies" },
            human_size(report.bytes)
        );
    } else {
        println!("rewind: nothing to clean");
    }
    if report.history_pruned > 0 {
        println!(
            "rewind: pruned {} old journal row{}",
            report.history_pruned,
            if report.history_pruned == 1 { "" } else { "s" }
        );
    }
    Ok(report)
}
