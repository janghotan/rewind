<div align="center">
  <img src="assets/icon.svg" width="96" alt="rewind">
  <h1>rewind</h1>
  <p><strong>Undo for destructive shell commands.</strong></p>
  <p><code>rm</code> · <code>mv</code> · <code>cp</code> · <code>chmod</code> · <code>chown</code> · destructive <code>git</code></p>

  [![CI](https://img.shields.io/github/actions/workflow/status/janghotan/rewind/ci.yml?style=flat-square&label=CI)](https://github.com/janghotan/rewind/actions/workflows/ci.yml)
  [![Release](https://img.shields.io/github/v/release/janghotan/rewind?style=flat-square&color=34d399)](https://github.com/janghotan/rewind/releases/latest)
  [![APT repository](https://img.shields.io/badge/APT-repository-34d399?style=flat-square)](https://janghotan.github.io/apt/)
  [![COPR](https://img.shields.io/badge/COPR-janghotan%2Frewind-51a2da?style=flat-square)](https://copr.fedorainfracloud.org/coprs/janghotan/rewind/)
  [![License: MIT](https://img.shields.io/badge/license-MIT-blue?style=flat-square)](LICENSE)

  <p><strong>English</strong> · <a href="README.zh-CN.md">简体中文</a></p>
  <p><a href="#quick-start">Quick start</a> · <a href="#usage">Usage</a> · <a href="#what-rewind-protects">Coverage</a> · <a href="#how-it-works">How it works</a> · <a href="#configuration">Configuration</a></p>
</div>

rewind sits between your shell and the commands that destroy state. It stages exactly what is about to change, runs the real command with its usual output and exit code, then keeps a local journal so you can reverse it later — one command, one file, or the last ten.

```console
$ rm -rf src/
$ rewind undo
rewind: undone #1 rm -rf src/
$ ls
src/
```

> [!NOTE]
> No daemon, no FUSE, no shim binaries. rewind is a single static binary plus a shell hook that stays out of the way until you need it.

## Why rewind

Everyone has done it: the wrong `rm -rf`, a `mv` that silently clobbered a config, `git reset --hard` on uncommitted work, a `chmod -R` that locked you out. Backup tools are too coarse, `trash-cli` only covers `rm`, and `git` only protects what you remembered to commit.

rewind keeps one undo stack across all of them.

| Capability | `trash-cli` | `git` | `rewind` |
|---|:---:|:---:|:---:|
| Undo `rm` | Yes | No | Yes |
| Undo `mv` / `cp` overwrites | No | No | Yes |
| Undo permission / owner changes | No | No | Yes |
| Undo `git reset --hard` | No | Partial | Yes |
| Undo `git clean` | No | No | Yes |
| Mixed-command undo stack | No | No | Yes |
| Works outside a repository | Yes | No | Yes |

## Quick start

### Install

The fastest install is the static Linux binary:

```bash
curl -fsSL https://github.com/janghotan/rewind/releases/latest/download/rewind-x86_64-unknown-linux-musl.tar.gz \
  | tar -xz -C ~/.local/bin
```

Replace `x86_64` with `aarch64` on ARM64. Release binaries are statically linked against musl and have no runtime dependencies.

<details>
<summary><strong>Package managers and source builds</strong></summary>

**Fedora / RHEL**

```bash
sudo dnf copr enable janghotan/rewind
sudo dnf install rewind
```

**Debian / Ubuntu**

```bash
sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://janghotan.github.io/apt/rewind-archive-keyring.gpg \
  | sudo tee /etc/apt/keyrings/rewind.gpg >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/rewind.gpg] https://janghotan.github.io/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/rewind.list
sudo apt update && sudo apt install rewind
```

The APT repository carries static `amd64` and `arm64` packages and works across Debian and Ubuntu releases. Browse it at <https://janghotan.github.io/apt/>.

Distro packages install the hooks under `/usr/share/rewind/` and completions in the standard shell directories.

**Cargo** requires Rust 1.85 or newer:

```bash
cargo install --git https://github.com/janghotan/rewind --locked
```

**From source**

```bash
git clone https://github.com/janghotan/rewind
cd rewind
cargo build --release
install -Dm755 target/release/rewind ~/.local/bin/rewind
```

</details>

### Enable the shell hook

Add one line to your shell's startup file, then restart the shell or source the file:

```bash
# bash
echo 'eval "$(rewind init bash)"' >> ~/.bashrc

# zsh
echo 'eval "$(rewind init zsh)"' >> ~/.zshrc

# fish
echo 'rewind init fish | source' >> ~/.config/fish/config.fish
```

The hook defines shell functions for the wrapped commands. They forward to `rewind exec`, which either stages the command or delegates straight to the real binary. Output, exit codes, prompts, and pipes behave exactly as before:

```console
$ rm missing.txt
rm: cannot remove 'missing.txt': No such file or directory
$ echo $?
1
```

Only the destructive `git` subcommands — `reset`, `clean`, `checkout`, and `restore` — are routed through rewind. Everything else, including `git status`, `git log`, and `git commit`, has zero overhead.

To uninstall, remove the line from your startup file and delete `~/.local/share/rewind/`.

## Usage

| Command | What it does |
|---|---|
| `rewind undo [N]` | Reverse the last `N` destructive commands, newest first |
| `rewind log` | Show the undo history; add `--json` for machines |
| `rewind status` | Show data locations, stack size, retention, and active wrappers; `--json` for machines |
| `rewind clean` | Purge staged data by age or by the configured size cap |

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

> [!IMPORTANT]
> Undo is resumable. If part of an entry cannot be restored — for example because you recreated a file at the same path — the successfully restored items are removed, the rest stay, and the command stops so older entries are not applied out of order. Resolve the conflict and run `rewind undo` again.

### `rewind log`

```console
$ rewind log
   ID  WHEN                 STATUS            SIZE  COMMAND
    2  2026-10-21 23:34:11  undone               -  chmod 600 app/config.yml
    1  2026-10-21 23:34:11  undoable          4 KiB  mv new-config.yml app/config.yml
```

`--json` emits machine-readable rows with the full command, working directory, status, item count, and staged bytes. `-n/--limit` controls how many rows are shown (default 30).

| Status | Meaning |
|---|---|
| `undoable` | Staged data is intact; `rewind undo` can reverse it |
| `undone` | Already reversed |
| `running` | The command is still executing, or was interrupted recently |
| `interrupted` | The wrapped command was killed before it finished |
| `failed` | The wrapped command failed; leftovers may still be staged |
| `expired` | Staged data was purged by `rewind clean` |

### `rewind status`

```console
$ rewind status
rewind 0.1.1

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

### Bypass

```bash
REWIND_DISABLE=1 rm -rf build/   # skip rewind for one command
command rm -rf build/            # bypass the shell function
/bin/rm -rf build/               # bypass everything
```

## What rewind protects

| Command | Staged before the command runs | Undo restores |
|---|---|---|
| `rm` | The operand itself, moved by atomic rename into the trash | Original path, contents, metadata, and timestamps |
| `mv` | Any destination that would be overwritten | Both sides of the overwrite; the moved file returns to its source |
| `cp` | Overwritten destination files/trees, plus a record of newly created paths | Previous destination contents; newly created paths are removed |
| `chmod` | Mode, and where relevant timestamps, of every target | Previous mode |
| `chown` | Owner and group of every target | Previous owner and group |
| `git reset --hard` | `HEAD`, the index tree, and all uncommitted changes, pinned under `refs/rewind/*` | The exact pre-reset worktree and index |
| `git clean -f…` | Hard-link snapshots of every candidate path | Deleted untracked/ignored files and directories |
| `git checkout -- <paths>` | Affected worktree files and the index tree | Previous worktree contents; index restored |
| `git restore` | Same as `checkout`, honoring `--staged` and `--source` | Previous worktree and/or index state |

`rm` implements GNU-compatible behavior for the common surface, including `-r/-R`, `-f`, `-i`, `-I`, `-d`, `-v`, `--`, `--preserve-root`, and `--no-preserve-root`. Flags rewind does not model (`--one-file-system`, `--help`, `--version`, unknown flags) are passed directly to the real `rm`, preserving native behavior at the cost of undo for that one call.

## How it works

1. **Stage.** rewind records what is about to be destroyed. For `rm`, this is an atomic `rename(2)` into the trash — undo is O(1) and byte-exact, even for a 20 GB directory. For overwrites it makes real copies, because the command may rewrite files in place. For `git clean` it uses hard links.
2. **Journal.** Each invocation gets a row in a local SQLite database with the command line, working directory, status, and per-item staging metadata.
3. **Run.** The real command inherits stdin/stdout/stderr, and its exit code is returned unchanged.
4. **Undo.** `rewind undo` walks the journal newest-first and restores staged items, deleting each journal item as it succeeds.

> [!WARNING]
> If staging is impossible — a read-only parent, no space, or an unsupported flag — rewind does not block the command. It falls back to native behavior and records the entry as non-undoable. A warning on a TTY tells you undo is unavailable for that call. Destructive commands never become less capable than the originals.

### Safety

- The data directory and its ancestors are never staged or deleted; `rm -rf ~/.local/share/rewind` is refused.
- Deleting `/` is refused unless the real tools would allow it; `rm` honors `--preserve-root`.
- Trash and per-entry directories are created with mode `0700`.
- Undo refuses to overwrite a path that has been recreated since the command ran; it reports the conflict instead of destroying new work.
- There is no network access and no telemetry. The journal is a local file.

### Performance

Staging an `rm` is a rename within the same filesystem: effectively free and independent of tree size. `cp`/`mv` overwrites copy only the destinations that will be overwritten. `git clean` hard-links candidates. The fixed overhead per wrapped command on a warm cache is one SQLite transaction plus process startup. `git` commands other than the four destructive subcommands do not touch rewind at all.

### Git snapshots

`git reset --hard` is the hardest case because it can destroy `HEAD`, the index, and uncommitted work at once. rewind creates a dangling commit with `git stash create`, pins `HEAD`, the index, and that stash under `refs/rewind/<id>-*` so `git gc` cannot collect them, then runs the real reset. `git clean` snapshots the candidate set and honors filters such as `-e`, `-X`, `-x`, `-d`, and pathspecs without reimplementing them. `checkout` and `restore` snapshot the affected worktree files and the index; branch switching, `--patch`, `--merge`, and interactive modes pass straight through.

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

| Variable | Purpose |
|---|---|
| `REWIND_DISABLE=1` | Bypass rewind for this invocation (handled by the shell hook) |
| `REWIND_DATA_DIR` | Override the data directory (`journal.db`, `trash/`) |
| `REWIND_JOURNAL` | Override the journal database path |
| `REWIND_CONFIG_PATH` | Override the config file path |
| `XDG_DATA_HOME` / `XDG_CONFIG_HOME` | Standard XDG overrides |

`REWIND_DATA_DIR`, `REWIND_JOURNAL`, and `REWIND_CONFIG_PATH` exist primarily for tests and sandboxed use.

## Limitations

These are deliberate: rewind prefers correct native behavior over a broken approximation.

- **Linux is the primary target.** macOS builds and works for the filesystem commands, but is tested less; see CI.
- **Non-UTF-8 arguments** are passed to the native command without undo for that invocation.
- **`rm` flags not modeled** (`--one-file-system`, unknown flags) pass through without undo.
- **`rm -r symlink/`** — a trailing slash on a symlink follows the link; rewind delegates rather than guessing.
- **Empty directories** deleted by `git clean` are not restored.
- **Submodule state** and `.git` internals beyond `HEAD`/index/worktree are not snapshotted.
- **Interrupted commands** (SIGKILL) may leave a `running` entry; after 24 hours it is marked `interrupted`, and whatever was staged can still be restored or cleaned.
- **No cross-user recovery.** The journal belongs to the user who ran the command.
- **Disk usage.** Deleted files occupy trash until `rewind clean` runs; retention is 30 days by default.

## Development

```bash
cargo build --release
cargo test          # unit + integration; hermetic, no network, no root
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The integration suite runs the real binary against temporary `HOME`/XDG directories and covers `rm`, `mv`, `cp`, `chmod`, `chown`, git operations, shell hooks, JSON output, config, expiry, and failure paths. See [CONTRIBUTING.md](CONTRIBUTING.md) for the test harness and release process, and the [man page](docs/rewind.1) for command details. Packaging lives in [`packaging/`](packaging/).

### Static Linux builds

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
cargo build --release --target aarch64-unknown-linux-musl
```

<p align="center"><sub>MIT licensed · <a href="https://github.com/janghotan/rewind">github.com/janghotan/rewind</a></sub></p>
