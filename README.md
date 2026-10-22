# rewind

**English** | [简体中文](README.zh-CN.md)

[![CI](https://github.com/janghotan/rewind/actions/workflows/ci.yml/badge.svg)](https://github.com/janghotan/rewind/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/janghotan/rewind?color=34d399)](https://github.com/janghotan/rewind/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Universal undo for destructive shell commands.**

`rm`, `mv`, `cp`, `chmod`, `chown`, and destructive `git` operations — reversed with one command, from any terminal, after the fact.

```console
$ rm -rf src/
$ rewind undo
rewind: undone #1 rm -rf src/
$ ls
src/
```

No daemon. No FUSE. No wrapper binary in your `PATH`. A single static binary and a shell hook that is invisible until you need it.

---

## Why

Everyone has done it: the wrong `rm -rf`, a `mv` that silently clobbered a config, `git reset --hard` on uncommitted work, a `chmod -R` that locked you out. Backup tools are too coarse, trash-cli only covers `rm`, and `git` only protects what you remembered to commit.

`rewind` sits between your shell and the destructive command. It stages the *pre-action state* of exactly what is about to change, runs the real command with its normal output and exit code, and keeps a journal so you can reverse it later — one command, one file, or the last ten.

| | `trash-cli` | `git` | `rewind` |
|---|---|---|---|
| `rm` undo | ✅ | ❌ | ✅ |
| `mv` / `cp` overwrite undo | ❌ | ❌ | ✅ |
| permission / owner undo | ❌ | ❌ | ✅ |
| `git reset --hard` undo | ❌ | partial | ✅ |
| `git clean` undo | ❌ | ❌ | ✅ |
| mixed-command undo stack | ❌ | ❌ | ✅ |
| works outside a repo | ✅ | ❌ | ✅ |

## Install

### Prebuilt binary (Linux)

```bash
curl -fsSL https://github.com/janghotan/rewind/releases/latest/download/rewind-x86_64-unknown-linux-musl.tar.gz \
  | tar -xz -C ~/.local/bin
```

Replace `x86_64` with `aarch64` on ARM64. The release binaries are statically linked against musl and have no runtime dependencies.

### Cargo

```bash
cargo install --git https://github.com/janghotan/rewind --locked
```

Requires Rust 1.85 or newer.

### Package managers

```bash
# Fedora / RHEL (COPR)
sudo dnf copr enable janghotan/rewind
sudo dnf install rewind

# Debian / Ubuntu (signed APT repository)
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://janghotan.github.io/apt/rewind-archive-keyring.gpg \
  | sudo tee /etc/apt/keyrings/rewind.gpg >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/rewind.gpg] https://janghotan.github.io/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/rewind.list
sudo apt update && sudo apt install rewind
```

The APT repository carries statically linked packages for `amd64` and
`arm64`, so it works across Debian and Ubuntu releases without runtime
dependencies. Browse it at <https://janghotan.github.io/apt/>.

Distro packages place the hooks in `/usr/share/rewind/` and shell completions in the standard directories. See [`packaging/`](packaging/) for building `.rpm` and `.deb` files yourself.

### From source

```bash
git clone https://github.com/janghotan/rewind
cd rewind
cargo build --release
install -Dm755 target/release/rewind ~/.local/bin/rewind
```

## Shell integration

Add one line to your shell's startup file, then restart the shell (or `source` it):

```bash
# bash
echo 'eval "$(rewind init bash)"' >> ~/.bashrc

# zsh
echo 'eval "$(rewind init zsh)"' >> ~/.zshrc

# fish
echo 'rewind init fish | source' >> ~/.config/fish/config.fish
```

The hook defines shell functions for the wrapped commands. They forward to `rewind exec`, which either stages and runs the command or delegates straight to the real binary. Ordinary output, exit codes, prompts, and pipes behave exactly as before:

```console
$ rm missing.txt
rm: cannot remove 'missing.txt': No such file or directory
$ echo $?
1
```

`git` is only routed through rewind for `reset`, `clean`, `checkout`, and `restore`; every other subcommand is untouched. `git status`, `git log`, `git commit`, and friends have zero overhead.

### Bypass

```bash
REWIND_DISABLE=1 rm -rf build/   # skip rewind for one command
command rm -rf build/            # bypass the shell function
/bin/rm -rf build/               # bypass everything
```

### Uninstall

Remove the line you added to your startup file. All state lives in `~/.local/share/rewind/` and can be deleted.

## Usage

```console
$ rewind --help
Undo for destructive shell commands

Usage: rewind <COMMAND>

Commands:
  undo         Reverse the last command, or the last N commands
  log          Show undo history
  clean        Purge staged files older than the retention period
  status       Show data locations, undo stack, and configuration summary
  init         Print the shell integration script
  completions  Print shell completions
  config       Inspect or change configuration
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### `rewind undo [N]`

Reverse the last `N` journal entries, newest first. Defaults to one.

```console
$ rm one; rm two; rm three
$ rewind undo 2
rewind: undone #3 rm three
rewind: undone #2 rm two
$ rewind undo --dry-run
would undo #1: rm one (1 item, 2 B)
```

Undo is **resumable**. If part of an entry cannot be restored (for example because you recreated a file at the same path), the successfully restored items are removed from the journal, the rest stay, and the command stops so older entries are not applied out of order. Resolve the conflict and run `rewind undo` again.

### `rewind log`

```console
$ rewind log
   ID  WHEN                 STATUS            SIZE  COMMAND
    2  2026-10-21 23:34:11  undone               -  chmod 600 app/config.yml
    1  2026-10-21 23:34:11  undoable          4 KiB  mv new-config.yml app/config.yml
```

`--json` emits machine-readable rows with the full command, working directory, status, item count, and staged bytes. `-n/--limit` controls how many rows are shown (default 30).

Status meanings:

| Status | Meaning |
|---|---|
| `undoable` | Staged data is intact; `rewind undo` can reverse it |
| `undone` | Already reversed |
| `running` | Command is still executing (or was interrupted very recently) |
| `interrupted` | The wrapped command was killed before it finished |
| `failed` | The wrapped command failed; leftovers may still be staged |
| `expired` | Staged data was purged by `rewind clean` |

### `rewind status`

```console
$ rewind status
rewind 0.1.0

  data      /home/you/.local/share/rewind
  config    /home/you/.config/rewind/config.toml
  journal   /home/you/.local/share/rewind/journal.db (48 KiB)
  trash     /home/you/.local/share/rewind/trash (12 KiB)

  undo      3 commands available
            oldest 2 hours ago
  history   17 total, 13 undone, 1 expired
  staged    12 KiB

  retention 30 days
  wrappers  rm mv cp chmod chown git
  git ops   reset clean checkout restore
  shell     bash hook found
```

### `rewind clean`

Purge staged data by age, or enforce the configured size cap (oldest entries first). The journal keeps the history row and marks the entry `expired`.

```console
$ rewind clean --dry-run
rewind: would purge 2 entries (18 MiB)
$ rewind clean --older-than 7d
rewind: purged 2 entries (18 MiB)
$ rewind clean --all
rewind: purged 5 entries (41 MiB)
```

Duration units: `s`, `m`, `h`, `d`, `w`.

## What gets captured

| Command | Staged before the command runs | Undo restores |
|---|---|---|
| `rm` | The operand itself, moved by atomic rename into the trash | Original path, contents, metadata, and timestamps |
| `mv` | Any destination that would be overwritten | Both sides of the overwrite; the moved file returns to its source |
| `cp` | Overwritten destination files/trees, plus a record of newly created paths | Previous destination contents; newly created paths are removed |
| `chmod` | Mode (and, where relevant, timestamps) of every target | Previous mode |
| `chown` | Owner and group of every target | Previous owner and group |
| `git reset --hard` | `HEAD`, the index tree, and all uncommitted changes, pinned under `refs/rewind/*` | The exact pre-reset worktree and index |
| `git clean -f…` | Hard-link snapshots of every candidate path | Deleted untracked/ignored files and directories |
| `git checkout -- <paths>` | Affected worktree files and the index tree | Previous worktree contents; index restored |
| `git restore` | Same as `checkout`, honoring `--staged` and `--source` | Previous worktree and/or index state |

`rm` implements GNU-compatible behavior for the common surface, including `-r/-R`, `-f`, `-i`, `-I`, `-d`, `-v`, `--`, `--preserve-root`, and `--no-preserve-root`. Flags rewind does not model (`--one-file-system`, `--help`, `--version`, unknown flags) are passed directly to the real `rm`, preserving native behavior at the cost of undo for that one call.

## How it works

```
       shell hook                    rewind exec                    real command
  ┌─────────────────┐          ┌──────────────────────┐          ┌──────────────┐
  │ rm() { rewind…} │──argv───▶│ parse & validate     │          │ /bin/rm      │
  └─────────────────┘          │ stage pre-state      │          └──────┬───────┘
                               │ journal entry (SQLite)│                │
                               │ run real command ─────┼────────────────┘
                               │ mark done / interrupted│
                               └──────────┬───────────┘
                                          │
                               ~/.local/share/rewind/
                               ├── journal.db
                               └── trash/<entry-id>/
```

1. **Stage.** rewind records what is about to be destroyed. For `rm`, this is an atomic `rename(2)` into the trash — undo is O(1) and byte-exact, even for a 20 GB directory. For overwrites it makes real copies (the command may rewrite files in place). For `git clean` it uses hard links, which is nearly free.
2. **Journal.** Each invocation gets a row in a local SQLite database with the command line, working directory, status, and per-item staging metadata.
3. **Run.** The real command inherits stdin/stdout/stderr and its exit code is returned unchanged.
4. **Undo.** `rewind undo` walks the journal newest-first and restores staged items, deleting each journal item as it succeeds.

If staging is impossible (read-only parent, no space, unsupported flag), rewind does **not** block the command: it falls back to native behavior and records the entry as non-undoable. A warning on a TTY tells you undo is unavailable for that call. Destructive commands never become less capable than the originals.

### Safety properties

- The data directory and its ancestors are never staged or deleted; `rm -rf ~/.local/share/rewind` is refused.
- Deleting `/` is refused unless the real tools would allow it; `rm` honors `--preserve-root`.
- Trash and per-entry directories are created with mode `0700`.
- Undo refuses to overwrite a path that has been recreated since the command ran; it reports the conflict instead of destroying new work.
- There is no network access and no telemetry. The journal is a local file.

### Performance

Staging an `rm` is a rename within the same filesystem: effectively free and independent of tree size. `cp`/`mv` overwrites copy only the destinations that will be overwritten. `git clean` hard-links candidates. The fixed overhead per wrapped command on a warm cache is a single SQLite transaction plus process startup; `git` commands other than the four destructive subcommands do not touch rewind at all.

## Configuration

Configuration is optional. Defaults enable every wrapper and keep trash for 30 days. The file lives at `~/.config/rewind/config.toml`.

```bash
rewind config init          # write defaults
rewind config show          # print effective config
rewind config get wrap.rm   # read one key
rewind config set wrap.rm false
rewind config edit          # $VISUAL / $EDITOR
```

```toml
[wrap]
rm = true
mv = true
cp = true
chmod = true
chown = true
git = true          # master switch for git interception
git_reset = true
git_clean = true
git_checkout = true
git_restore = true

[trash]
retention_days = 30 # 0 disables automatic expiry
max_bytes = 0       # 0 = unlimited; oldest entries pruned first

[journal]
keep_days = 0       # 0 keeps history rows forever

[git]
snapshot_untracked = true  # snapshot untracked files before git clean
keep_refs = false          # keep refs/rewind/* after an entry is purged
```

Setting a wrapper to `false` is a true no-op: `rewind exec` detects it before opening the journal and executes the real command directly.

### Environment variables

| Variable | Purpose |
|---|---|
| `REWIND_DISABLE=1` | Bypass rewind for this invocation (handled by the shell hook) |
| `REWIND_DATA_DIR` | Override the data directory (`journal.db`, `trash/`) |
| `REWIND_JOURNAL` | Override the journal database path |
| `REWIND_CONFIG_PATH` | Override the config file path |
| `XDG_DATA_HOME` / `XDG_CONFIG_HOME` | Standard XDG overrides |

`REWIND_DATA_DIR`, `REWIND_JOURNAL`, and `REWIND_CONFIG_PATH` exist primarily for tests and sandboxed use.

## Git behavior in detail

`git reset --hard` is the hardest case because it can destroy three different things at once: `HEAD`, the index, and uncommitted worktree changes. rewind:

1. resolves the current `HEAD` and index tree;
2. creates a dangling commit of the worktree with `git stash create` (which does not modify anything);
3. pins all three objects under `refs/rewind/<id>-{head,index,stash}` so `git gc` cannot collect them;
4. runs the real `git reset --hard`;
5. on undo, checks out the saved worktree and restores the index, then removes the refs.

`git clean` snapshots the candidate set before the command and discards snapshots of paths that survived. This means filters such as `-e`, `-X`, `-x`, `-d`, and pathspecs are honored without rewind having to reimplement them. Empty directories are not preserved (Git does not track them).

`git checkout`/`git restore` snapshot the affected worktree files and the index tree. `--staged`, `--source`, `--worktree`, and pathspecs are supported. Branch switching, `--patch`, `--merge`, and interactive modes pass straight through without journaling.

## Limitations

These are deliberate: rewind prefers correct native behavior over a broken approximation.

- **Linux is the primary target.** macOS builds and works for the filesystem commands, but is tested less; see CI.
- **Non-UTF-8 arguments** are passed to the native command without undo for that invocation.
- **`rm` flags not modeled** (`--one-file-system`, unknown flags) pass through without undo.
- **`rm -r symlink/`** (trailing slash on a symlink) follows the link; rewind delegates rather than guessing.
- **Empty directories** deleted by `git clean` are not restored.
- **Submodule state** and `.git` internals beyond `HEAD`/index/worktree are not snapshotted.
- **Interrupted commands** (SIGKILL) may leave a `running` entry; after 24 hours it is marked `interrupted` and whatever was staged can still be restored or cleaned.
- **No cross-user recovery.** The journal belongs to the user who ran the command.
- **Disk usage.** Deleted files occupy trash until `rewind clean` runs; retention is 30 days by default.

## Building and testing

```bash
cargo build --release
cargo test          # unit + integration; hermetic, no network, no root
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The integration suite runs the real binary against temporary `HOME`/XDG directories and covers `rm`, `mv`, `cp`, `chmod`, `chown`, git operations, shell hooks, JSON output, config, expiry, and failure paths. See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the test harness and release process.

### Static Linux builds

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

## License

MIT. See [LICENSE](LICENSE).
