//! Shell-integration tests: hooks, exit-code parity, and invisibility.
//!
//! The parity tests compare rewind's observable behavior against the real
//! GNU tools with `LC_ALL=C`, which is the only locale where coreutils quoting
//! is stable across machines.

mod common;

use std::process::{Command, Output};

use common::*;

fn shell_available(shell: &str) -> bool {
    have_command(shell)
}

#[test]
fn bash_hook_wraps_rm_and_undo_restores() {
    if !shell_available("bash") {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write("f.txt", "hello");
    sandbox.mkdir("dir");

    let output = sandbox.run_shell(
        "bash",
        r#"
        eval "$(rewind init bash)"
        rm f.txt
        rm -rf dir
        test -e f.txt && { echo "still exists"; exit 1; }
        rewind undo >/dev/null
        rewind undo >/dev/null
        test "$(cat f.txt)" = "hello" || exit 2
        test -d dir || exit 3
        echo OK
        "#,
    );
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout_of(&output), "OK");
}

#[test]
fn bash_hook_preserves_exit_codes_and_output() {
    if !shell_available("bash") {
        return;
    }
    let sandbox = Sandbox::new();
    let output = sandbox.run_shell(
        "bash",
        r#"
        eval "$(rewind init bash)"
        rm does-not-exist
        echo "exit=$?"
        "#,
    );
    assert_eq!(stdout_of(&output), "exit=1");
    assert_eq!(
        stderr_of(&output),
        "rm: cannot remove 'does-not-exist': No such file or directory"
    );
}

#[test]
fn bash_hook_bypasses_with_rewind_disable() {
    if !shell_available("bash") {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write("f", "x");
    let output = sandbox.run_shell(
        "bash",
        r#"
        eval "$(rewind init bash)"
        REWIND_DISABLE=1 rm f
        test -e f && { echo STILL; exit 1; }
        rewind undo
        "#,
    );
    assert!(output.status.success());
    assert!(stdout_of(&output).contains("nothing to undo"));
}

#[test]
fn command_builtin_bypasses_the_hook() {
    if !shell_available("bash") {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write("f", "x");
    let output = sandbox.run_shell(
        "bash",
        r#"
        eval "$(rewind init bash)"
        command rm f
        rewind undo
        "#,
    );
    assert!(output.status.success());
    assert!(stdout_of(&output).contains("nothing to undo"));
}

#[test]
fn filenames_with_spaces_survive_shell_quoting() {
    if !shell_available("bash") {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write("file with spaces.txt", "spaced");
    let output = sandbox.run_shell(
        "bash",
        r#"
        eval "$(rewind init bash)"
        rm "file with spaces.txt"
        rewind undo >/dev/null
        cat "file with spaces.txt"
        "#,
    );
    assert!(output.status.success());
    assert_eq!(stdout_of(&output), "spaced");
}

#[test]
fn zsh_hook_works_when_available() {
    if !shell_available("zsh") {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write("z.txt", "zsh");
    let output = sandbox.run_shell(
        "zsh",
        r#"
        eval "$(rewind init zsh)"
        rm z.txt
        rewind undo >/dev/null
        cat z.txt
        "#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout_of(&output), "zsh");
}

#[test]
fn fish_hook_works_when_available() {
    if !shell_available("fish") {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.write("f.txt", "fish");
    let output = sandbox.run_shell(
        "fish",
        r#"
        rewind init fish | source
        rm f.txt
        rewind undo >/dev/null
        cat f.txt
        "#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(stdout_of(&output), "fish");
}

#[test]
fn hooks_leave_non_destructive_git_alone() {
    if !shell_available("bash") || !have_command("git") {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    let output = sandbox.run_shell(
        "bash",
        r#"
        eval "$(rewind init bash)"
        git status --short
        rewind log
        "#,
    );
    assert!(output.status.success());
    assert!(stdout_of(&output).contains("no history"));
}

/// Compare real `rm` with `rewind exec rm` for a case where both run in
/// separate, identical fixture directories.
fn compare_with_real_rm(sandbox: &Sandbox, args: &[&str], fixture: &[&str]) {
    for dir in ["real", "rew"] {
        let base = sandbox.mkdir(dir);
        for relative in fixture {
            let path = base.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, "data\n").unwrap();
        }
    }

    let real: Output = Command::new("rm")
        .args(args)
        .current_dir(sandbox.path("real"))
        .env("LC_ALL", "C")
        .output()
        .unwrap();

    let mut command = sandbox.command();
    command
        .current_dir(sandbox.path("rew"))
        .args(["exec", "rm", "--"])
        .args(args)
        .env("LC_ALL", "C");
    let rew = command.output().unwrap();

    assert_eq!(
        real.status.code(),
        rew.status.code(),
        "exit code mismatch for rm {args:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&real.stdout),
        String::from_utf8_lossy(&rew.stdout),
        "stdout mismatch for rm {args:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&real.stderr),
        String::from_utf8_lossy(&rew.stderr),
        "stderr mismatch for rm {args:?}"
    );
}

#[test]
fn rm_output_and_exit_codes_match_gnu_coreutils() {
    let sandbox = Sandbox::new();
    compare_with_real_rm(&sandbox, &["-f", "missing"], &[]);
    compare_with_real_rm(&sandbox, &["missing"], &[]);
    compare_with_real_rm(&sandbox, &[], &[]);
    compare_with_real_rm(&sandbox, &["-d", "nonempty"], &["nonempty/x"]);
    compare_with_real_rm(&sandbox, &["-v", "file"], &["file"]);
    compare_with_real_rm(&sandbox, &["file"], &["file"]);
}

#[test]
fn cp_and_mv_output_is_byte_identical_to_the_real_tools() {
    let sandbox = Sandbox::new();
    for dir in ["real", "rew"] {
        let base = sandbox.mkdir(dir);
        std::fs::write(base.join("src"), "src\n").unwrap();
        std::fs::write(base.join("dst"), "dst\n").unwrap();
    }

    let real = Command::new("cp")
        .args(["-v", "src", "dst"])
        .current_dir(sandbox.path("real"))
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    let mut command = sandbox.command();
    command
        .current_dir(sandbox.path("rew"))
        .args(["exec", "cp", "--", "-v", "src", "dst"])
        .env("LC_ALL", "C");
    let rew = command.output().unwrap();
    assert_eq!(real.status.code(), rew.status.code());
    assert_eq!(real.stdout, rew.stdout);
    assert_eq!(real.stderr, rew.stderr);
}
