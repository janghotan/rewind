# Changelog

All notable changes to this project are documented here. The format is based
on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-10-21

First public release.

### Added

- **Shell integration** for bash, zsh, and fish via `rewind init <shell>`.
  Wrapped commands preserve native stdout/stderr, stdin handling, prompts, and
  exit codes. `REWIND_DISABLE=1`, `command`, and absolute paths bypass the
  hook.
- **`rm` undo** implemented as an atomic rename into a per-entry trash
  directory. Supports `-r/-R`, `-f`, `-i`, `-I`, `-d`, `-v`, `--`,
  `--preserve-root`, and `--no-preserve-root` with GNU-compatible messages and
  exit codes. Cross-device staging falls back to copy-then-remove.
- **`mv` undo** that snapshots overwritten destinations, reconciles which
  moves actually happened, and restores both sides on undo.
- **`cp` undo** that snapshots overwritten destinations and records newly
  created paths so they can be removed. Supports recursive trees,
  multi-source copies, `-t`, and `--no-clobber`.
- **`chmod` and `chown` undo** that snapshot prior modes and ownership
  (recursively with `-R`) and restore them.
- **Git undo**:
  - `git reset --hard` — snapshots `HEAD`, the index tree, and uncommitted
    worktree changes with `git stash create`, pinned under `refs/rewind/*`.
  - `git clean -f…` — hard-link snapshots of every candidate path, honoring
    `-d`, `-x`, `-X`, `-e`, and pathspecs.
  - `git checkout -- <paths>` and `git restore` — worktree file snapshots plus
    index tree restoration, including `--staged`, `--source`, and
    `--worktree`.
- **Journal** backed by SQLite with per-item staging metadata, statuses
  (`running`, `done`, `undone`, `failed`, `interrupted`, `expired`), and
  resumable undo.
- **Commands**: `undo [N] [--dry-run]`, `log [--json] [-n]`,
  `status [--json]`, `clean [--older-than] [--all] [--dry-run]`,
  `init`, `completions`, and `config {path,show,get,set,init,edit}`.
- **Configuration** at `~/.config/rewind/config.toml`: per-command wrapper
  toggles, per-git-operation toggles, trash retention and size cap, journal
  retention, and git snapshot options. Changes take effect on the next
  command.
- **Safety guards**: refuses to stage the data directory or its ancestors,
  refuses to overwrite recreated paths during undo, creates private `0700`
  state directories, and degrades to native behavior when staging fails.
- **Test suite**: 117 unit and integration tests covering the wrapped
  commands, shell hooks, git round trips, JSON output, config, expiry, and
  failure paths.
- **CI**: build, format, clippy, and tests on push and pull request; static
  musl binaries for Linux x86_64 and aarch64 plus GitHub Releases on tags;
  automatic publication of signed APT packages after a release.
- **Packaging**: RPM spec for Fedora/COPR, Debian packaging for
  Debian/Ubuntu, and a signed APT repository with static `amd64`/`arm64`
  packages at <https://janghotan.github.io/apt/>.

[Unreleased]: https://github.com/janghotan/rewind/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/janghotan/rewind/releases/tag/v0.1.0
