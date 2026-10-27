//! User-facing commands: `log`, `status`, and `clean`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::journal::{Entry, EntryStatus, Journal, StagedItem, StagedKind, Stats};
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

/// Summarize the current state of one journal entry without changing it.
///
/// Inspection is intentionally conservative: it reports what is staged and
/// what conflicts exist right now, but never claims that undo is guaranteed.
pub fn inspect(journal: &Journal, _paths: &Paths, entry_id: i64, as_json: bool) -> Result<()> {
    let entry = journal
        .entry(entry_id)?
        .ok_or_else(|| Error::InvalidInput(format!("no journal entry #{entry_id}")))?;
    let staged = journal.staged_for(entry_id)?;
    let (item_count, staged_bytes) = journal.staged_summary(entry_id)?;
    let items: Vec<StagedView> = staged.iter().map(view_staged).collect();
    let recovery = analyze(&entry, &items);

    if as_json {
        let out = json!({
            "id": entry.id,
            "created_at": entry.created_at,
            "when": format_timestamp(entry.created_at),
            "finished_at": entry.finished_at,
            "command": entry.command_line(),
            "command_name": entry.command,
            "argv": entry.argv,
            "cwd": entry.cwd,
            "status": entry.status.as_str(),
            "undoable": entry.is_undoable(),
            "exit_code": entry.exit_code,
            "items": item_count,
            "staged_bytes": staged_bytes,
            "recovery": {
                "state": recovery.state,
                "message": recovery.message,
                "conflicts": recovery.conflicts,
                "missing": recovery.missing,
                "note": INSPECTION_NOTE,
            },
            "staged_items": items.iter().map(item_json).collect::<Vec<_>>(),
        });
        let text = serde_json::to_string_pretty(&out)
            .map_err(|err| Error::Journal(format!("serializing inspection: {err}")))?;
        println!("{text}");
        return Ok(());
    }

    print_inspection(&entry, &items, item_count, staged_bytes, &recovery);
    Ok(())
}

const INSPECTION_NOTE: &str =
    "inspection is read-only and cannot guarantee recovery; undo re-checks every destination";
const INSPECT_ITEM_LIMIT: usize = 50;
const TREE_SCAN_LIMIT: u64 = 10_000;

#[derive(Debug, Default, Clone)]
struct TreeSummary {
    files: u64,
    directories: u64,
    symlinks: u64,
    bytes: u64,
    truncated: bool,
}

struct StagedView {
    id: i64,
    origin: String,
    kind: StagedKind,
    stored: Option<String>,
    bytes: u64,
    data_available: bool,
    object_type: &'static str,
    mode: Option<String>,
    mtime: Option<i64>,
    contents: Option<TreeSummary>,
}

struct RecoveryView {
    state: &'static str,
    message: String,
    conflicts: Vec<String>,
    missing: Vec<String>,
}

fn view_staged(item: &StagedItem) -> StagedView {
    let stored = if item.stored.is_empty() {
        None
    } else {
        Some(item.stored.clone())
    };
    // Payload-backed kinds are inspected through the staged copy. Created and
    // empty-directory markers describe the original path instead.
    let payload_backed = matches!(
        item.kind,
        StagedKind::Move | StagedKind::Copy | StagedKind::Metadata
    );
    let target = if payload_backed {
        stored.as_deref().map(PathBuf::from)
    } else {
        Some(PathBuf::from(&item.origin))
    };

    let mut data_available = true;
    let mut object_type = "missing";
    let mut mode = None;
    let mut mtime = None;
    let mut contents = None;

    if item.kind == StagedKind::Directory {
        object_type = "directory";
        contents = Some(TreeSummary::default());
    } else if let Some(target) = &target {
        match metadata_of(target) {
            Some(meta) => {
                object_type = object_type_of(&meta);
                mode = mode_of(&meta);
                mtime = meta.modified().ok().and_then(|time| {
                    time.duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .map(|delta| delta.as_secs() as i64)
                });
                if meta.file_type().is_dir() {
                    contents = Some(summarize_tree(target));
                }
            }
            None => {
                if payload_backed {
                    data_available = false;
                }
            }
        }
    }

    StagedView {
        id: item.id,
        origin: item.origin.clone(),
        kind: item.kind,
        stored,
        bytes: item.size,
        data_available,
        object_type,
        mode,
        mtime,
        contents,
    }
}

fn metadata_of(path: &Path) -> Option<fs::Metadata> {
    fs::symlink_metadata(path).ok()
}

fn object_type_of(meta: &fs::Metadata) -> &'static str {
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        "symlink"
    } else if file_type.is_dir() {
        "directory"
    } else if file_type.is_file() {
        "file"
    } else {
        "special"
    }
}

fn mode_of(meta: &fs::Metadata) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(format!("{:04o}", meta.permissions().mode() & 0o7777))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

/// Count the contents of a tree without following symlinks. Stops after
/// `TREE_SCAN_LIMIT` entries so inspecting a huge directory stays responsive.
fn summarize_tree(root: &Path) -> TreeSummary {
    let mut summary = TreeSummary::default();
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0u64;
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries {
            if seen >= TREE_SCAN_LIMIT {
                summary.truncated = true;
                return summary;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            seen += 1;
            let path = entry.path();
            let Some(meta) = metadata_of(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                summary.symlinks += 1;
            } else if meta.file_type().is_dir() {
                summary.directories += 1;
                stack.push(path);
            } else {
                summary.files += 1;
                summary.bytes = summary.bytes.saturating_add(meta.len());
            }
        }
    }
    summary
}

fn analyze(entry: &Entry, items: &[StagedView]) -> RecoveryView {
    let mut conflicts = Vec::new();
    let mut missing = Vec::new();

    if entry.command == "git_reset" {
        match entry.meta.get("repo").and_then(Value::as_str) {
            Some(repo) if Path::new(repo).is_dir() => {}
            Some(repo) => missing.push(format!("repository {repo} no longer exists")),
            None => missing.push("entry has no repository path".into()),
        }
        if entry.meta.get("head").and_then(Value::as_str).is_none() {
            missing.push("entry has no HEAD snapshot".into());
        }
    } else if entry.command == "git_branch_delete" {
        let (branch_conflicts, branch_missing) = crate::engine::git::branch_delete_blockers(entry);
        conflicts.extend(branch_conflicts);
        missing.extend(branch_missing);
    } else if entry.command == "mv" {
        analyze_mv(entry, &mut conflicts, &mut missing);
        for item in items {
            if item.kind == StagedKind::Metadata && !item.data_available {
                missing.push(format!("{}: staged data is missing", item.origin));
            }
        }
    } else {
        for item in items {
            if !item.data_available {
                missing.push(format!("{}: staged data is missing", item.origin));
                continue;
            }
            match (entry.command.as_str(), item.kind) {
                ("rm", StagedKind::Move) | ("git_clean", StagedKind::Copy) => {
                    if metadata_of(Path::new(&item.origin)).is_some() {
                        conflicts.push(format!(
                            "{} already exists; undo refuses to overwrite it",
                            item.origin
                        ));
                    }
                }
                ("git_clean", StagedKind::Directory) => {
                    if let Some(meta) = metadata_of(Path::new(&item.origin)) {
                        if !meta.file_type().is_dir() {
                            conflicts.push(format!(
                                "{} is now a non-directory entry; undo cannot create the directory",
                                item.origin
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let snapshot_backed = matches!(
        entry.command.as_str(),
        "git_reset" | "git_branch_delete" | "mv"
    );
    if entry.is_undoable() && items.is_empty() && !snapshot_backed {
        missing.push("entry has no staged items".into());
    }

    let (state, message) = if entry.status == EntryStatus::Undone {
        ("undone", "this entry was already undone")
    } else if entry.status == EntryStatus::Expired {
        ("expired", "staged data was purged by `rewind clean`")
    } else if entry.status == EntryStatus::Failed {
        (
            "failed",
            "the wrapped command reported failure; there may be nothing to undo",
        )
    } else if entry.status == EntryStatus::Running {
        (
            "running",
            "the wrapped command is still running, or was interrupted before recording a result",
        )
    } else if !entry.undoable {
        ("not-undoable", "this entry was not staged for undo")
    } else if !missing.is_empty() {
        (
            "missing-staged-data",
            "some staged data is missing; undo cannot fully restore this entry",
        )
    } else if !conflicts.is_empty() {
        (
            "conflict",
            "undo will refuse conflicting destinations until they are moved away",
        )
    } else if entry.status == EntryStatus::Interrupted {
        (
            "ready",
            "staged data is available after an interrupted command and may be partial",
        )
    } else {
        ("ready", "ready to undo")
    };

    RecoveryView {
        state,
        message: message.to_string(),
        conflicts,
        missing,
    }
}

fn analyze_mv(entry: &Entry, conflicts: &mut Vec<String>, missing: &mut Vec<String>) {
    let Some(pairs) = entry.meta.get("pairs").and_then(Value::as_array) else {
        missing.push("entry has no recorded move pairs".into());
        return;
    };
    for pair in pairs {
        let src = pair.get("src").and_then(Value::as_str).unwrap_or_default();
        let dest = pair.get("dest").and_then(Value::as_str).unwrap_or_default();
        let moved = pair.get("moved").and_then(Value::as_bool).unwrap_or(false);
        if !moved {
            continue;
        }
        let src_exists = metadata_of(Path::new(src)).is_some();
        let dest_exists = metadata_of(Path::new(dest)).is_some();
        if src_exists {
            conflicts.push(format!(
                "{src} already exists; undo cannot move {dest} back"
            ));
        } else if !dest_exists {
            missing.push(format!("both {src} and {dest} are missing"));
        }
    }
}

fn print_inspection(
    entry: &Entry,
    items: &[StagedView],
    item_count: u64,
    staged_bytes: u64,
    recovery: &RecoveryView,
) {
    let status = if entry.is_undoable() {
        format!("{} (undoable)", entry.status.as_str())
    } else {
        entry.status.as_str().to_string()
    };
    println!("Entry #{}", entry.id);
    println!("  Status:       {status}");
    println!("  Command:      {}", entry.command_line());
    println!("  Directory:    {}", entry.cwd);
    println!("  Created:      {}", format_timestamp(entry.created_at));
    if let Some(finished_at) = entry.finished_at {
        println!("  Finished:     {}", format_timestamp(finished_at));
    }
    if let Some(code) = entry.exit_code {
        println!("  Exit code:    {code}");
    }
    println!("  Items:        {item_count}");
    println!("  Staged size:  {}", human_size(staged_bytes));
    println!();

    if items.is_empty() {
        println!("Staged items: none");
    } else {
        println!("Staged items:");
        for item in items.iter().take(INSPECT_ITEM_LIMIT) {
            println!("  {}", item.origin);
            println!("    Kind:      {}", item.kind.as_str());
            println!("    Type:      {}", item.object_type);
            if let Some(stored) = &item.stored {
                println!(
                    "    Stored:    {stored}{}",
                    if item.data_available {
                        ""
                    } else {
                        " (missing)"
                    }
                );
            }
            println!("    Size:      {}", human_size(item.bytes));
            if let Some(mode) = &item.mode {
                println!("    Mode:      {mode}");
            }
            if let Some(mtime) = item.mtime {
                println!("    Modified:  {}", format_timestamp(mtime));
            }
            match (item.kind, &item.contents) {
                (StagedKind::Directory, _) => {
                    println!("    Contents:  empty directory");
                }
                (_, Some(summary)) => {
                    println!("    Contents:  {}", describe_contents(summary));
                }
                _ => {}
            }
        }
        if items.len() > INSPECT_ITEM_LIMIT {
            println!(
                "  ... and {} more item{} (use --json for the full list)",
                items.len() - INSPECT_ITEM_LIMIT,
                if items.len() - INSPECT_ITEM_LIMIT == 1 {
                    ""
                } else {
                    "s"
                }
            );
        }
    }
    println!();

    println!("Recovery:");
    println!("  State:        {}", recovery.state);
    println!("  Details:      {}", recovery.message);
    if !recovery.conflicts.is_empty() {
        println!("  Conflicts:");
        for conflict in &recovery.conflicts {
            println!("    - {conflict}");
        }
    }
    if !recovery.missing.is_empty() {
        println!("  Missing:");
        for missing in &recovery.missing {
            println!("    - {missing}");
        }
    }
    println!("  Note:         {INSPECTION_NOTE}");
}

fn describe_contents(summary: &TreeSummary) -> String {
    let mut parts = Vec::new();
    if summary.files > 0 {
        parts.push(format!(
            "{} file{}",
            summary.files,
            if summary.files == 1 { "" } else { "s" }
        ));
    }
    if summary.directories > 0 {
        parts.push(format!(
            "{} director{}",
            summary.directories,
            if summary.directories == 1 { "y" } else { "ies" }
        ));
    }
    if summary.symlinks > 0 {
        parts.push(format!(
            "{} symlink{}",
            summary.symlinks,
            if summary.symlinks == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        parts.push("empty".to_string());
    }
    let mut text = parts.join(", ");
    text.push_str(&format!(" ({})", human_size(summary.bytes)));
    if summary.truncated {
        text.push_str(" (partial count)");
    }
    text
}

fn item_json(item: &StagedView) -> Value {
    json!({
        "id": item.id,
        "origin": item.origin,
        "kind": item.kind.as_str(),
        "stored": item.stored,
        "bytes": item.bytes,
        "data_available": item.data_available,
        "object_type": item.object_type,
        "mode": item.mode,
        "mtime": item.mtime,
        "contents": item.contents.as_ref().map(|summary| json!({
            "files": summary.files,
            "directories": summary.directories,
            "symlinks": summary.symlinks,
            "bytes": summary.bytes,
            "truncated": summary.truncated,
        })),
    })
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
            ("branch", config.wrap.git_branch_delete),
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
