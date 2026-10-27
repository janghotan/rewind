//! Git destructive-operation interception.
//!
//! Three different strategies are used because the commands destroy different
//! things:
//!
//! * `git reset --hard` rewrites refs, the index, and the worktree. rewind
//!   snapshots HEAD, the index tree, and uncommitted changes (via
//!   `git stash create`, which does not mutate anything) and pins them under
//!   `refs/rewind/*` so garbage collection cannot collect them.
//! * `git clean -f…` deletes untracked/ignored paths. rewind hard-link
//!   snapshots the candidates first, runs the real command, then discards
//!   snapshots for paths that survived.
//! * `git checkout -- <paths>` / `git restore` overwrite worktree files (and
//!   sometimes the index). rewind snapshots the affected files and the index
//!   tree, then restores both on undo.
//!
//! Anything rewind cannot model with confidence (interactive mode, `--patch`,
//! backup/exchange modes, branch switching) is passed straight through.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::json;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::journal::{Entry, EntryStatus, Journal, NewStaged, StagedKind};
use crate::paths::Paths;
use crate::staging::{SnapshotMode, Stager};

use super::{run_passthrough, warn};

/// Entry point dispatched from the shell wrapper.
pub fn execute(
    journal: &mut Journal,
    paths: &Paths,
    config: &Config,
    argv: &[String],
) -> Result<i32> {
    let Some(subcommand) = argv.first() else {
        return run_passthrough("git", argv);
    };
    if !config.wraps_git_op(subcommand) {
        return run_passthrough("git", argv);
    }
    match subcommand.as_str() {
        "reset" => reset(journal, paths, config, argv),
        "clean" => clean(journal, paths, config, argv),
        "checkout" => checkout(journal, paths, argv),
        "restore" => restore(journal, paths, argv),
        "branch" => branch(journal, paths, config, argv),
        _ => run_passthrough("git", argv),
    }
}

struct Repo {
    root: PathBuf,
    cwd: PathBuf,
}

impl Repo {
    fn discover() -> Option<Self> {
        let cwd = std::env::current_dir().ok()?;
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&cwd)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let root = String::from_utf8(output.stdout).ok()?;
        let root = PathBuf::from(root.trim());
        if root.as_os_str().is_empty() {
            return None;
        }
        Some(Self { root, cwd })
    }

    /// Run git in the user's original working directory.
    fn git(&self, args: &[&str]) -> Result<Output> {
        Command::new("git")
            .args(args)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| Error::Git(format!("running git: {err}")))
    }

    /// Run git in the repository root.
    fn git_root(&self, args: &[&str]) -> Result<Output> {
        Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| Error::Git(format!("running git: {err}")))
    }

    fn text(&self, args: &[&str]) -> Result<Option<String>> {
        let output = self.git(args)?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|_| Error::Git("git produced non-UTF-8 output".into()))?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            Ok(Some(trimmed.to_string()))
        }
    }

    /// Parse NUL-separated output into repository-relative paths.
    fn z_paths(&self, args: &[&str]) -> Result<Vec<String>> {
        let output = self.git(args)?;
        if !output.status.success() {
            return Err(Error::Git(format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let raw = String::from_utf8(output.stdout)
            .map_err(|_| Error::Git("git produced non-UTF-8 paths".into()))?;
        Ok(raw
            .split('\0')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn absolute(&self, relative: &str) -> Option<PathBuf> {
        let path = Path::new(relative);
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return None;
        }
        Some(self.root.join(path))
    }
}

// ---------------------------------------------------------------------------
// git reset --hard
// ---------------------------------------------------------------------------

fn reset(journal: &mut Journal, _paths: &Paths, config: &Config, argv: &[String]) -> Result<i32> {
    let destructive = argv
        .iter()
        .skip(1)
        .any(|arg| arg == "--hard" || (arg.starts_with('-') && arg.contains('h') && arg.len() > 2));
    if !destructive {
        return run_passthrough("git", argv);
    }
    let Some(repo) = Repo::discover() else {
        return run_passthrough("git", argv);
    };

    let head = match repo.text(&["rev-parse", "--verify", "--quiet", "HEAD"])? {
        Some(head) => head,
        // Unborn branch: there is no prior commit to return to.
        None => return run_passthrough("git", argv),
    };
    let branch = repo.text(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let index_tree = repo.text(&["write-tree"])?;
    let stash = repo.text(&["stash", "create"])?;

    let entry_id = journal.begin_entry("git_reset", argv, &repo.cwd)?;

    let mut refs: Vec<String> = Vec::new();
    if let Err(err) = pin_ref(&repo, entry_id, "head", &head) {
        warn(&format!("could not pin HEAD snapshot: {err}"));
    } else {
        refs.push(ref_name(entry_id, "head"));
    }
    if let Some(tree) = &index_tree {
        if pin_ref(&repo, entry_id, "index", tree).is_ok() {
            refs.push(ref_name(entry_id, "index"));
        }
    }
    if let Some(stash) = &stash {
        if pin_ref(&repo, entry_id, "stash", stash).is_ok() {
            refs.push(ref_name(entry_id, "stash"));
        }
    }

    let exit_code = run_passthrough("git", argv)?;

    let meta = json!({
        "repo": repo.root.display().to_string(),
        "head": head,
        "branch": branch,
        "index_tree": index_tree,
        "stash": stash,
        "refs": refs,
        "keep_refs": config.git.keep_refs,
    });
    journal.finish_entry(
        entry_id,
        EntryStatus::Done,
        Some(exit_code),
        true,
        Some(&meta),
    )?;
    Ok(exit_code)
}

fn ref_name(entry_id: i64, suffix: &str) -> String {
    format!("refs/rewind/{entry_id}-{suffix}")
}

fn pin_ref(repo: &Repo, entry_id: i64, suffix: &str, object: &str) -> Result<()> {
    let name = ref_name(entry_id, suffix);
    let output = repo.git_root(&["update-ref", &name, object])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Git(format!(
            "update-ref {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Restore a `git reset --hard` snapshot. Called by `undo`.
pub fn undo_reset(entry: &Entry) -> Result<()> {
    let repo = entry
        .meta
        .get("repo")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Git("reset entry has no repository path".into()))?;
    let head = entry
        .meta
        .get("head")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Git("reset entry has no HEAD snapshot".into()))?;
    let repo_path = PathBuf::from(repo);
    if !repo_path.exists() {
        return Err(Error::Git(format!("repository no longer exists: {repo}")));
    }

    let branch = entry.meta.get("branch").and_then(|v| v.as_str());
    if let Some(branch) = branch {
        let branch_exists = git_succeeds(
            &repo_path,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        );
        if branch_exists {
            git_must(&repo_path, &["checkout", "-q", "-f", branch])?;
        } else {
            git_must(&repo_path, &["checkout", "-q", "-f", "--detach", head])?;
        }
    } else {
        git_must(&repo_path, &["checkout", "-q", "-f", "--detach", head])?;
    }
    git_must(&repo_path, &["reset", "--hard", head])?;

    // `git stash apply --index` restores both the worktree and the index. Only
    // fall back to read-tree when there were no tracked modifications to
    // reapply (a staged-only change is represented by the index tree).
    if let Some(stash) = entry.meta.get("stash").and_then(|v| v.as_str()) {
        let output = Command::new("git")
            .args(["stash", "apply", "--index", stash])
            .current_dir(&repo_path)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| Error::Git(format!("running git stash apply: {err}")))?;
        if !output.status.success() {
            return Err(Error::Git(format!(
                "could not reapply uncommitted changes: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
    } else if let Some(tree) = entry.meta.get("index_tree").and_then(|v| v.as_str()) {
        git_must(&repo_path, &["read-tree", tree])?;
    }

    if !entry
        .meta
        .get("keep_refs")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        if let Some(refs) = entry.meta.get("refs").and_then(|v| v.as_array()) {
            for reference in refs.iter().filter_map(|v| v.as_str()) {
                let _ = Command::new("git")
                    .args(["update-ref", "-d", reference])
                    .current_dir(&repo_path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
    Ok(())
}

fn git_must(repo: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| Error::Git(format!("running git {}: {err}", args.join(" "))))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Git(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn git_succeeds(repo: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Restore a saved index tree (used by checkout/restore undo).
pub fn undo_index(repo: &Path, tree: &str) -> Result<()> {
    git_must(repo, &["read-tree", tree])
}

// ---------------------------------------------------------------------------
// git clean
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IgnoreMode {
    None,
    AlsoIgnored,
    OnlyIgnored,
}

struct CleanOptions {
    ignore: IgnoreMode,
    force: bool,
    dry_run: bool,
    interactive: bool,
    directories: bool,
    excludes: Vec<String>,
    pathspecs: Vec<String>,
}

fn clean(journal: &mut Journal, paths: &Paths, _config: &Config, argv: &[String]) -> Result<i32> {
    let Some(options) = parse_clean(&argv[1..]) else {
        return run_passthrough("git", argv);
    };
    if !options.force || options.dry_run || options.interactive {
        return run_passthrough("git", argv);
    }
    let Some(repo) = Repo::discover() else {
        return run_passthrough("git", argv);
    };

    // Candidates: untracked (and ignored when -x/-X applies). Listing is done
    // with NUL separation so filenames with newlines or quotes are safe.
    let mut candidates: Vec<String> = Vec::new();
    if options.ignore != IgnoreMode::OnlyIgnored {
        candidates.extend(list_untracked(&repo, &options, false)?);
    }
    if options.ignore != IgnoreMode::None {
        candidates.extend(list_ignored(&repo, &options, false)?);
    }
    candidates.sort();
    candidates.dedup();

    // `ls-files --directory` collapses entirely untracked directories into a
    // single entry. Those directories may contain nested empty directories
    // that the file candidates above cannot describe, so walk them and record
    // the empty leaves explicitly.
    let mut directory_candidates: Vec<String> = Vec::new();
    if options.directories {
        if options.ignore != IgnoreMode::OnlyIgnored {
            directory_candidates.extend(list_untracked(&repo, &options, true)?);
        }
        if options.ignore != IgnoreMode::None {
            directory_candidates.extend(list_ignored(&repo, &options, true)?);
        }
        directory_candidates.retain(|path| path.ends_with('/'));
        directory_candidates.sort();
        directory_candidates.dedup();
    }

    let entry_id = journal.begin_entry("git_clean", argv, &repo.cwd)?;
    let stager = Stager::new(paths, entry_id);
    let mut rows: Vec<(i64, PathBuf)> = Vec::new();
    let mut failures = 0usize;

    let mut empty_dirs: Vec<PathBuf> = Vec::new();
    for relative in &directory_candidates {
        let relative = relative.trim_end_matches('/');
        let Some(absolute) = repo.absolute(relative) else {
            continue;
        };
        if let Err(err) = collect_empty_dirs(&absolute, &mut empty_dirs) {
            failures += 1;
            warn(&format!(
                "could not inspect {} before git clean: {err}",
                absolute.display()
            ));
        }
    }
    empty_dirs.sort();
    empty_dirs.dedup();
    for dir in empty_dirs {
        let mode = std::fs::symlink_metadata(&dir)
            .ok()
            .and_then(|meta| mode_bits(&meta));
        let item = NewStaged::new(dir.display().to_string(), "", StagedKind::Directory)
            .with_meta(json!({"role": "git_clean", "mode": mode}));
        match journal.add_staged(entry_id, &item) {
            Ok(row) => rows.push((row, dir)),
            Err(err) => {
                failures += 1;
                warn(&format!("could not record empty directory: {err}"));
            }
        }
    }

    for relative in &candidates {
        let Some(absolute) = repo.absolute(relative) else {
            continue;
        };
        if absolute.symlink_metadata().is_err() {
            continue;
        }
        match stager.snapshot(&absolute, SnapshotMode::Hardlink) {
            Ok(payload) => {
                let item = NewStaged::new(
                    absolute.display().to_string(),
                    payload.stored.display().to_string(),
                    StagedKind::Copy,
                )
                .with_size(payload.size)
                .with_meta(json!({"role": "git_clean"}));
                match journal.add_staged(entry_id, &item) {
                    Ok(row) => rows.push((row, absolute)),
                    Err(err) => {
                        failures += 1;
                        let _ = stager.discard_stored(&payload.stored);
                        warn(&format!("could not record clean snapshot: {err}"));
                    }
                }
            }
            Err(err) => {
                failures += 1;
                warn(&format!(
                    "could not snapshot {} before git clean: {err}",
                    absolute.display()
                ));
            }
        }
    }

    let exit_code = run_passthrough("git", argv)?;

    // Anything that survived the clean does not need an undo snapshot.
    for (row, absolute) in rows {
        if absolute.symlink_metadata().is_ok() {
            if let Ok(items) = journal.staged_for(entry_id) {
                if let Some(item) = items.iter().find(|item| item.id == row) {
                    let stored = PathBuf::from(&item.stored);
                    let _ = stager.discard_stored(&stored);
                }
            }
            journal.remove_staged(row)?;
        }
    }

    let undoable = journal.staged_count(entry_id)? > 0;
    if undoable || failures > 0 {
        journal.finish_entry(
            entry_id,
            EntryStatus::Done,
            Some(exit_code),
            undoable,
            Some(&json!({"repo": repo.root.display().to_string(), "role": "clean", "failures": failures})),
        )?;
    } else {
        journal.delete_entry(entry_id)?;
        stager.cleanup_entry_dir()?;
    }
    Ok(exit_code)
}

fn list_untracked(repo: &Repo, options: &CleanOptions, directories: bool) -> Result<Vec<String>> {
    let mut args: Vec<String> = vec![
        "ls-files".into(),
        "-z".into(),
        "--full-name".into(),
        "--others".into(),
        "--exclude-standard".into(),
    ];
    if directories {
        args.push("--directory".into());
    }
    for exclude in &options.excludes {
        args.push(format!("--exclude={exclude}"));
    }
    if !options.pathspecs.is_empty() {
        args.push("--".into());
        args.extend(options.pathspecs.iter().cloned());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    repo.z_paths(&refs)
}

fn list_ignored(repo: &Repo, options: &CleanOptions, directories: bool) -> Result<Vec<String>> {
    let mut args: Vec<String> = vec![
        "ls-files".into(),
        "-z".into(),
        "--full-name".into(),
        "--others".into(),
        "--ignored".into(),
        "--exclude-standard".into(),
    ];
    if directories {
        args.push("--directory".into());
    }
    for exclude in &options.excludes {
        args.push(format!("--exclude={exclude}"));
    }
    if !options.pathspecs.is_empty() {
        args.push("--".into());
        args.extend(options.pathspecs.iter().cloned());
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    repo.z_paths(&refs)
}

/// Collect directories below `dir` that contain no files (only empty subdirectories
/// or no entries at all).
fn collect_empty_dirs(dir: &Path, out: &mut Vec<PathBuf>) -> Result<bool> {
    let mut entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(crate::error::io_err(dir, err)),
    };
    let mut contains_file = false;
    for entry in &mut entries {
        let entry = entry.map_err(|err| crate::error::io_err(dir, err))?;
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(crate::error::io_err(&path, err)),
        };
        if meta.file_type().is_dir() {
            let sub_empty = collect_empty_dirs(&path, out)?;
            if !sub_empty {
                contains_file = true;
            }
        } else {
            contains_file = true;
        }
    }
    if !contains_file {
        out.push(dir.to_path_buf());
        Ok(true)
    } else {
        Ok(false)
    }
}

fn mode_bits(meta: &std::fs::Metadata) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Some(meta.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        None
    }
}

fn parse_clean(args: &[String]) -> Option<CleanOptions> {
    let mut options = CleanOptions {
        ignore: IgnoreMode::None,
        force: false,
        dry_run: false,
        interactive: false,
        directories: false,
        excludes: Vec::new(),
        pathspecs: Vec::new(),
    };
    let mut iter = args.iter();
    let mut only_paths = false;
    while let Some(arg) = iter.next() {
        if only_paths {
            options.pathspecs.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_paths = true;
            continue;
        }
        if !arg.starts_with('-') {
            options.pathspecs.push(arg.clone());
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            match name {
                "force" => options.force = true,
                "dry-run" => options.dry_run = true,
                "interactive" => options.interactive = true,
                "quiet" => {}
                "exclude" => options
                    .excludes
                    .push(inline.or_else(|| iter.next().cloned())?),
                _ => return None,
            }
            continue;
        }
        let mut chars = arg.chars().skip(1).peekable();
        while let Some(flag) = chars.next() {
            match flag {
                'f' => options.force = true,
                'n' => options.dry_run = true,
                'i' => options.interactive = true,
                'q' => {}
                'd' => options.directories = true,
                'x' => options.ignore = IgnoreMode::AlsoIgnored,
                'X' => options.ignore = IgnoreMode::OnlyIgnored,
                'e' => {
                    let rest: String = chars.by_ref().collect();
                    options.excludes.push(if rest.is_empty() {
                        iter.next().cloned()?
                    } else {
                        rest
                    });
                }
                _ => return None,
            }
        }
    }
    Some(options)
}

// ---------------------------------------------------------------------------
// git branch -d / -D
// ---------------------------------------------------------------------------

/// Snapshot of one branch that is about to be deleted.
struct BranchSnapshot {
    name: String,
    commit: String,
}

fn branch(journal: &mut Journal, _paths: &Paths, config: &Config, argv: &[String]) -> Result<i32> {
    let Some(names) = parse_branch_delete(&argv[1..]) else {
        return run_passthrough("git", argv);
    };
    if names.is_empty() {
        return run_passthrough("git", argv);
    }
    let Some(repo) = Repo::discover() else {
        return run_passthrough("git", argv);
    };

    // Resolve every name before anything is deleted. Names that do not resolve
    // are left to the native command, which reports the familiar error.
    let mut snapshots = Vec::new();
    for name in &names {
        if let Some(commit) = rev_parse_branch(&repo.root, name) {
            snapshots.push(BranchSnapshot {
                name: name.clone(),
                commit,
            });
        }
    }
    if snapshots.is_empty() {
        return run_passthrough("git", argv);
    }

    let entry_id = journal.begin_entry("git_branch_delete", argv, &repo.cwd)?;
    let mut pinned = Vec::with_capacity(snapshots.len());
    let mut failures = 0usize;
    let mut branches = Vec::new();

    for (index, snapshot) in snapshots.iter().enumerate() {
        let reference = ref_name(entry_id, &format!("branch-{index}"));
        match update_ref(&repo.root, &reference, &snapshot.commit) {
            Ok(()) => pinned.push(true),
            Err(err) => {
                pinned.push(false);
                failures += 1;
                warn(&format!(
                    "could not pin branch {} before deletion: {err}",
                    snapshot.name
                ));
            }
        }
        branches.push(json!({
            "name": snapshot.name,
            "commit": snapshot.commit,
            "ref": reference,
        }));
    }

    let exit_code = run_passthrough("git", argv)?;

    // A branch that still exists was not deleted (validation failure, current
    // branch, hook rejection, ...). Drop its pin and its undo promise.
    let mut surviving = Vec::new();
    let mut restorable = Vec::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        let reference = ref_name(entry_id, &format!("branch-{index}"));
        if rev_parse_branch(&repo.root, &snapshot.name).is_some() {
            let _ = delete_ref(&repo.root, &reference);
            surviving.push(snapshot.name.clone());
        } else if pinned[index] {
            restorable.push(branches[index].clone());
        }
    }

    if restorable.is_empty() && surviving.is_empty() {
        // Nothing was deleted and nothing failed to pin: no entry is useful.
        journal.delete_entry(entry_id)?;
        return Ok(exit_code);
    }
    if restorable.is_empty() && failures == 0 {
        journal.delete_entry(entry_id)?;
        return Ok(exit_code);
    }

    let undoable = !restorable.is_empty();
    let meta = json!({
        "repo": repo.root.display().to_string(),
        "role": "branch_delete",
        "branches": restorable,
        "survived": surviving,
        "keep_refs": config.git.keep_refs,
        "failures": failures,
    });
    journal.finish_entry(
        entry_id,
        EntryStatus::Done,
        Some(exit_code),
        undoable,
        Some(&meta),
    )?;
    Ok(exit_code)
}

fn parse_branch_delete(args: &[String]) -> Option<Vec<String>> {
    let mut delete = false;
    let mut remotes = false;
    let mut names = Vec::new();
    let mut only_names = false;
    for arg in args {
        if only_names {
            names.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_names = true;
            continue;
        }
        if !arg.starts_with('-') {
            names.push(arg.clone());
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            match long {
                "delete" => delete = true,
                "remotes" => remotes = true,
                "force" => {}
                // Listing, moving, copying, tracking, and interactive modes
                // are not modeled.
                _ => return None,
            }
            continue;
        }
        for flag in arg.chars().skip(1) {
            match flag {
                'd' | 'D' => delete = true,
                'r' => remotes = true,
                'f' => {}
                _ => return None,
            }
        }
    }
    if !delete || remotes {
        return None;
    }
    Some(names)
}

fn rev_parse_branch(repo: &Path, name: &str) -> Option<String> {
    let output = Command::new("git")
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ])
        .current_dir(repo)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn update_ref(repo: &Path, name: &str, object: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["update-ref", name, object])
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| Error::Git(format!("running git update-ref: {err}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Git(format!(
            "update-ref {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn delete_ref(repo: &Path, name: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["update-ref", "-d", name])
        .current_dir(repo)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| Error::Git(format!("running git update-ref: {err}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Git(format!(
            "deleting {name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

/// Restore branches deleted by a `git branch -d` / `-D` entry.
///
/// Returns `true` when every recorded branch exists again. A branch that was
/// recreated at a different commit is reported as a conflict and leaves the
/// entry undoable; no ref is overwritten.
pub fn undo_branch_delete(entry: &Entry) -> Result<bool> {
    let repo = entry
        .meta
        .get("repo")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Git("branch entry has no repository path".into()))?;
    let repo_path = PathBuf::from(repo);
    if !repo_path.is_dir() {
        return Err(Error::Git(format!("repository no longer exists: {repo}")));
    }
    let branches = entry
        .meta
        .get("branches")
        .and_then(|v| v.as_array())
        .ok_or_else(|| Error::Git("branch entry has no recorded branches".into()))?;
    let keep_refs = entry
        .meta
        .get("keep_refs")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut success = true;
    for branch in branches {
        let Some(name) = branch.get("name").and_then(|v| v.as_str()) else {
            success = false;
            continue;
        };
        let Some(commit) = branch.get("commit").and_then(|v| v.as_str()) else {
            success = false;
            continue;
        };
        let reference = branch
            .get("ref")
            .and_then(|v| v.as_str())
            .unwrap_or_default();

        match rev_parse_branch(&repo_path, name) {
            Some(current) if current == commit => {
                if !keep_refs {
                    let _ = delete_ref(&repo_path, reference);
                }
            }
            Some(_) => {
                eprintln!(
                    "rewind: branch {name} already exists at a different commit; \
                     delete or move it before retrying"
                );
                success = false;
            }
            None => match update_ref(&repo_path, &format!("refs/heads/{name}"), commit) {
                Ok(()) => {
                    println!("rewind: restored branch {name}");
                    if !keep_refs {
                        let _ = delete_ref(&repo_path, reference);
                    }
                }
                Err(err) => {
                    eprintln!("rewind: restoring branch {name}: {err}");
                    success = false;
                }
            },
        }
    }
    Ok(success)
}

/// Read-only conflict detection for `rewind inspect`.
pub fn branch_delete_blockers(entry: &Entry) -> (Vec<String>, Vec<String>) {
    let mut conflicts = Vec::new();
    let mut missing = Vec::new();
    let Some(repo) = entry.meta.get("repo").and_then(|v| v.as_str()) else {
        missing.push("entry has no repository path".into());
        return (conflicts, missing);
    };
    if !Path::new(repo).is_dir() {
        missing.push(format!("repository {repo} no longer exists"));
        return (conflicts, missing);
    }
    let Some(branches) = entry.meta.get("branches").and_then(|v| v.as_array()) else {
        missing.push("entry has no recorded branches".into());
        return (conflicts, missing);
    };
    for branch in branches {
        let Some(name) = branch.get("name").and_then(|v| v.as_str()) else {
            missing.push("entry contains a malformed branch record".into());
            continue;
        };
        let commit = branch.get("commit").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(current) = rev_parse_branch(Path::new(repo), name) {
            if current != commit {
                let short = current.get(..7).unwrap_or(&current);
                conflicts.push(format!(
                    "branch {name} already exists at {short}; undo will not overwrite it"
                ));
            }
        }
    }
    (conflicts, missing)
}

// ---------------------------------------------------------------------------
// git checkout / git restore
// ---------------------------------------------------------------------------

fn checkout(journal: &mut Journal, paths: &Paths, argv: &[String]) -> Result<i32> {
    let Some(separator) = argv.iter().position(|arg| arg == "--") else {
        // Branch switching, -b, etc.: not destructive to tracked content in a
        // way we can snapshot reliably.
        return run_passthrough("git", argv);
    };
    let pathspecs: Vec<String> = argv[separator + 1..].to_vec();
    if pathspecs.is_empty() {
        return run_passthrough("git", argv);
    }
    let before = &argv[1..separator];
    if before
        .iter()
        .any(|arg| arg == "-p" || arg == "--patch" || arg == "--ours" || arg == "--theirs")
    {
        return run_passthrough("git", argv);
    }
    let treeish = before
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str);

    snapshot_paths_command(
        journal,
        paths,
        argv,
        "git_checkout",
        &pathspecs,
        treeish,
        treeish.is_some(),
        true,
    )
}

fn restore(journal: &mut Journal, paths: &Paths, argv: &[String]) -> Result<i32> {
    let Some(options) = parse_restore(&argv[1..]) else {
        return run_passthrough("git", argv);
    };
    if options.pathspecs.is_empty() {
        return run_passthrough("git", argv);
    }
    let staged = options.staged;
    let worktree = options.worktree || !options.staged;
    snapshot_paths_command(
        journal,
        paths,
        argv,
        "git_restore",
        &options.pathspecs,
        options.source.as_deref(),
        staged,
        worktree,
    )
}

struct RestoreOptions {
    staged: bool,
    worktree: bool,
    source: Option<String>,
    pathspecs: Vec<String>,
}

fn parse_restore(args: &[String]) -> Option<RestoreOptions> {
    let mut options = RestoreOptions {
        staged: false,
        worktree: false,
        source: None,
        pathspecs: Vec::new(),
    };
    let mut iter = args.iter();
    let mut only_paths = false;
    while let Some(arg) = iter.next() {
        if only_paths {
            options.pathspecs.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_paths = true;
            continue;
        }
        if !arg.starts_with('-') {
            options.pathspecs.push(arg.clone());
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (long, None),
            };
            match name {
                "staged" => options.staged = true,
                "worktree" => options.worktree = true,
                "source" => options.source = Some(inline.or_else(|| iter.next().cloned())?),
                "quiet" | "progress" | "no-progress" | "ignore-unmerged" | "overlay"
                | "no-overlay" => {}
                // Interactive or conflict-side restoration is not modeled.
                _ => return None,
            }
            continue;
        }
        let mut chars = arg.chars().skip(1).peekable();
        while let Some(flag) = chars.next() {
            match flag {
                'S' => options.staged = true,
                'W' => options.worktree = true,
                'q' => {}
                's' => {
                    let rest: String = chars.by_ref().collect();
                    options.source = Some(if rest.is_empty() {
                        iter.next().cloned()?
                    } else {
                        rest
                    });
                }
                _ => return None,
            }
        }
    }
    Some(options)
}

#[allow(clippy::too_many_arguments)]
fn snapshot_paths_command(
    journal: &mut Journal,
    paths: &Paths,
    argv: &[String],
    command: &str,
    pathspecs: &[String],
    treeish: Option<&str>,
    index_updated: bool,
    worktree_updated: bool,
) -> Result<i32> {
    let Some(repo) = Repo::discover() else {
        return run_passthrough("git", argv);
    };

    // Only tracked files can be affected by checkout/restore.
    let mut files = list_tracked_matching(&repo, pathspecs)?;
    if let Some(treeish) = treeish {
        files.extend(list_tree_matching(&repo, treeish, pathspecs)?);
    }
    files.sort();
    files.dedup();

    let index_tree = if index_updated {
        match repo.text(&["write-tree"])? {
            Some(tree) => Some(tree),
            // An unmerged index cannot be snapshotted faithfully.
            None => return run_passthrough("git", argv),
        }
    } else {
        None
    };

    let entry_id = journal.begin_entry(command, argv, &repo.cwd)?;
    let stager = Stager::new(paths, entry_id);
    let mut staged_rows: Vec<(i64, PathBuf, bool)> = Vec::new();
    let mut failures = 0usize;

    if worktree_updated {
        for relative in &files {
            let Some(absolute) = repo.absolute(relative) else {
                continue;
            };
            if absolute.symlink_metadata().is_ok() {
                match stager.snapshot(&absolute, SnapshotMode::Copy) {
                    Ok(payload) => {
                        let item = NewStaged::new(
                            absolute.display().to_string(),
                            payload.stored.display().to_string(),
                            StagedKind::Copy,
                        )
                        .with_size(payload.size)
                        .with_meta(json!({"role": command}));
                        match journal.add_staged(entry_id, &item) {
                            Ok(row) => staged_rows.push((row, absolute, false)),
                            Err(err) => {
                                failures += 1;
                                let _ = stager.discard_stored(&payload.stored);
                                warn(&format!("could not record git snapshot: {err}"));
                            }
                        }
                    }
                    Err(err) => {
                        failures += 1;
                        warn(&format!("could not snapshot {}: {err}", absolute.display()));
                    }
                }
            } else {
                // The file is tracked but missing; checkout will recreate it.
                let item = NewStaged::new(absolute.display().to_string(), "", StagedKind::Created);
                match journal.add_staged(entry_id, &item) {
                    Ok(row) => staged_rows.push((row, absolute, true)),
                    Err(err) => {
                        failures += 1;
                        warn(&format!("could not record missing file: {err}"));
                    }
                }
            }
        }
    }

    let exit_code = run_passthrough("git", argv)?;

    // Drop "created" markers for files that were not actually created.
    for (row, absolute, created) in &staged_rows {
        if *created && absolute.symlink_metadata().is_err() {
            journal.remove_staged(*row)?;
        }
    }

    let undoable = journal.staged_count(entry_id)? > 0 || index_tree.is_some();
    if undoable || failures > 0 {
        journal.finish_entry(
            entry_id,
            EntryStatus::Done,
            Some(exit_code),
            undoable,
            Some(&json!({
                "repo": repo.root.display().to_string(),
                "index_tree": index_tree,
                "role": command,
                "snapshot_failures": failures,
            })),
        )?;
    } else {
        journal.delete_entry(entry_id)?;
        stager.cleanup_entry_dir()?;
    }
    Ok(exit_code)
}

fn list_tracked_matching(repo: &Repo, pathspecs: &[String]) -> Result<Vec<String>> {
    let mut args: Vec<String> = vec![
        "ls-files".into(),
        "-z".into(),
        "--full-name".into(),
        "--".into(),
    ];
    args.extend(pathspecs.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    repo.z_paths(&refs)
}

fn list_tree_matching(repo: &Repo, treeish: &str, pathspecs: &[String]) -> Result<Vec<String>> {
    let mut args: Vec<String> = vec![
        "ls-tree".into(),
        "-r".into(),
        "-z".into(),
        "--full-name".into(),
        "--name-only".into(),
        treeish.into(),
        "--".into(),
    ];
    args.extend(pathspecs.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    repo.z_paths(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn clean_parses_combined_flags() {
        let options = parse_clean(&args(&["-fdx", "target"])).unwrap();
        assert!(options.force);
        assert_eq!(options.ignore, IgnoreMode::AlsoIgnored);
        assert_eq!(options.pathspecs, vec!["target"]);
    }

    #[test]
    fn clean_parses_excludes() {
        let options = parse_clean(&args(&["-f", "-e", "*.log", "-e", "tmp", "dir"])).unwrap();
        assert_eq!(options.excludes, vec!["*.log", "tmp"]);
        assert_eq!(options.pathspecs, vec!["dir"]);
    }

    #[test]
    fn restore_parses_modes_and_source() {
        let options = parse_restore(&args(&["--source", "HEAD~1", "--worktree", "src"])).unwrap();
        assert_eq!(options.source.as_deref(), Some("HEAD~1"));
        assert!(options.worktree);
        assert!(!options.staged);
        assert_eq!(options.pathspecs, vec!["src"]);
    }

    #[test]
    fn restore_rejects_interactive() {
        assert!(parse_restore(&args(&["-p", "file"])).is_none());
    }

    #[test]
    fn clean_marks_interactive_mode_for_passthrough() {
        let options = parse_clean(&args(&["-i"])).unwrap();
        assert!(options.interactive);
    }
}
