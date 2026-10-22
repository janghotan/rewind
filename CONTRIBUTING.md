# Contributing to rewind

Thanks for taking the time to help. This document covers the development
setup, the architecture, the test harness, and what a good pull request looks
like.

## Ground rules

- Be kind. Assume good intent. Critique code, not people.
- Every behavior change needs a test that fails without it.
- Keep the invisible-wrapper promise: rewind must never change a command's
  output, exit code, prompts, or stream behavior. If a feature cannot preserve
  those, it belongs in the passthrough path.
- Prefer correctness over cleverness. When rewind cannot model a case
  faithfully, delegate to the real binary rather than approximating.

## Development setup

Requires Rust 1.85+ and a C toolchain (for the bundled SQLite used by
`rusqlite`).

```bash
git clone https://github.com/janghotan/rewind
cd rewind
cargo build
cargo test
```

Useful commands:

```bash
cargo fmt                       # format
cargo clippy --all-targets -- -D warnings
cargo test --release            # catches timing/race issues
cargo test rm::                # filter by test name
cargo test -- --nocapture      # show test stdout
```

## Trying it by hand

The binary reads `HOME` and the XDG variables, so a sandbox is trivial:

```bash
export HOME=/tmp/rewind-sandbox
export XDG_DATA_HOME=$HOME/.local/share
export XDG_CONFIG_HOME=$HOME/.config
mkdir -p $HOME && cd $HOME

eval "$(/home/jangho/Projects/rewind/target/debug/rewind init bash)"
mkdir -p demo && echo hi > demo/file
rm -r demo
rewind undo
rewind log
rewind status
```

Use `REWIND_DISABLE=1` to compare against native behavior, and
`REWIND_DATA_DIR`/`REWIND_JOURNAL`/`REWIND_CONFIG_PATH` when a test needs
isolated state.

## Architecture

```
src/
├── main.rs           CLI (clap), dispatch, passthrough
├── lib.rs            library surface used by tests
├── paths.rs          XDG layout and private directories
├── config.rs         config load/save/validate
├── journal.rs        SQLite schema, entries, staged items, stats
├── staging.rs        rename/copy/hardlink staging and restore
├── undo.rs           resumable undo dispatch
├── commands.rs       log / status / clean
├── engine/
│   ├── mod.rs        dispatch, warnings, passthrough, exit codes
│   ├── rm.rs         GNU-compatible rm on top of trash renames
│   ├── transfer.rs   shared mv/cp argument parsing and pair resolution
│   ├── mv.rs         destination snapshots + move reconciliation
│   ├── cp.rs         destination snapshots + created-path tracking
│   ├── chmod.rs      mode snapshots
│   ├── chown.rs      ownership snapshots
│   ├── metadata.rs   recursive mode/owner collection and restore
│   └── git.rs        reset/clean/checkout/restore snapshots
└── util.rs           time, size, and duration helpers
```

`shell/` contains the bash, zsh, and fish hooks embedded with `include_str!`
and printed by `rewind init`.

### Journal model

A journal entry is one wrapped invocation. It stores the command, argv, cwd,
timestamps, status, and JSON metadata. Each entry owns zero or more staged
items:

- `Move` — the original path was renamed into `trash/<id>/…` (`rm`).
- `Copy` — a snapshot copy or hardlink of a path that may be destroyed
  (`mv`, `cp`, `git clean`, `git checkout`, `git restore`).
- `Created` — a path that did not exist before and should be removed on undo
  (`cp`, `git checkout`, `git restore`).
- `Metadata` — a JSON snapshot of modes or ownership (`chmod`, `chown`).

Undo deletes each staged row as soon as it is restored, which makes partial
undo resumable and prevents double restoration.

### The passthrough rule

`run_passthrough` executes the real command with inherited stdio and returns
its exit code. rewind uses it whenever:

- the wrapper is disabled in config or `REWIND_DISABLE` is set;
- arguments are not valid UTF-8;
- the command line uses a flag rewind does not model;
- staging is impossible (read-only parent, ENOSPC, permission denied);
- a git invocation is not one of the four destructive subcommands.

Passthrough means "native behavior, no undo". It must never mean "refuse to
run the command".

## Tests

The integration tests under `tests/` are hermetic: each test gets a temporary
`HOME`, XDG directories, and a `PATH`, and invokes the compiled binary. No
network, no root, no dependency on the developer's real state.

```
tests/common/mod.rs   sandbox harness (Command builders, env isolation)
tests/rm.rs           rm semantics, recursive trees, prompts, cross-device
tests/transfer.rs     mv/cp overwrites, created paths, multi-source, failures
tests/git.rs          reset/clean/checkout/restore round trips, subdirs
tests/cli.rs          log/status/clean/config/init/completions/JSON
tests/shell.rs        hook behavior in bash, zsh, and fish
```

When adding a test:

1. Use the harness rather than hand-rolling paths.
2. Assert on observable filesystem state, not on internal structs.
3. Cover the undo path, not just the forward command.
4. Add a failure-path test when the feature can conflict (for example,
   recreating a path before undo).

Unit tests live next to the code and cover parsing, config validation, path
resolution, and staging primitives.

## Adding a wrapped command

1. Add the module under `src/engine/` and register it in `engine::dispatch`.
2. Add config switches in `config.rs` (`wrap.<cmd>`, plus `known_keys` and
   `set`).
3. Add the shell function in all three hooks. Non-destructive git
   subcommands must stay on the fast path.
4. Add a `StagedKind` undo branch in `undo.rs` if the new command needs one.
5. Add integration tests for: success + undo, failure + no broken state,
   disabled wrapper, and an unmodeled flag that must pass through.
6. Update the tables in `README.md`.

## Pull requests

- One logical change per PR. Keep commits focused and messages in the
  imperative mood (`rm: preserve timestamps on cross-device staging`).
- Include a short "why" in the PR description, plus before/after output when
  behavior is user-visible.
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, and
  `cargo test` before pushing. CI runs the same on Linux.
- New user-facing behavior needs a `CHANGELOG.md` entry under `Unreleased`.
- Do not bump the version in a feature PR; maintainers do that at release
  time.

## Release process

Releases are driven by tags:

```bash
# 1. Update Cargo.toml version and CHANGELOG.md
# 2. Commit, then:
git tag -a v0.1.1 -m "rewind 0.1.1"
git push origin main --tags
```

The `release` workflow builds static musl binaries for
`x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`, creates a
GitHub Release, and attaches the archives plus checksums. Packaging files in
`packaging/` and `debian/` consume the same tag.

## Reporting bugs

Include:

- `rewind --version`, OS, and shell;
- the exact command you ran (redact paths if needed);
- `rewind log` and `rewind status` output;
- whether `REWIND_DISABLE=1 <command>` behaves correctly.

A minimal reproduction in a temporary directory is worth more than a long
description.

## License

By contributing, you agree that your contributions are licensed under the
MIT License (see `LICENSE`).
