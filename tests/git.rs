//! End-to-end tests for git interception.
//!
//! Every test is a no-op when git is unavailable.

mod common;

use common::*;

fn git_available() -> bool {
    have_command("git")
}

#[test]
fn reset_hard_undo_restores_branch_and_files() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("tracked.txt", "v2\n");
    sandbox.git_ok(&["commit", "-qam", "second"]);

    sandbox.exec_ok("git", &["reset", "--hard", "HEAD~1"]);
    assert_eq!(sandbox.read("tracked.txt"), "v1\n");
    assert!(
        sandbox
            .git_ok(&["rev-parse", "--short", "HEAD"])
            .trim()
            .len()
            >= 7
    );

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("tracked.txt"), "v2\n");
    assert!(
        sandbox
            .git_ok(&["log", "-1", "--format=%s"])
            .contains("second")
    );
}

#[test]
fn reset_hard_undo_restores_staged_and_unstaged_changes() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("tracked.txt", "staged\n");
    sandbox.git_ok(&["add", "tracked.txt"]);
    sandbox.write("tracked.txt", "staged\nunstaged\n");
    assert!(sandbox.git_ok(&["status", "--porcelain"]).contains("MM"));

    sandbox.exec_ok("git", &["reset", "--hard", "HEAD"]);
    assert_eq!(sandbox.read("tracked.txt"), "v1\n");

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("tracked.txt"), "staged\nunstaged\n");
    assert!(sandbox.git_ok(&["status", "--porcelain"]).contains("MM"));
}

#[test]
fn reset_hard_leaves_untracked_files_and_undo_keeps_them() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("scratch.txt", "untracked");

    sandbox.exec_ok("git", &["reset", "--hard", "HEAD"]);
    assert_eq!(sandbox.read("scratch.txt"), "untracked");
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("scratch.txt"), "untracked");
}

#[test]
fn reset_soft_is_not_intercepted() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("tracked.txt", "v2\n");
    sandbox.git_ok(&["commit", "-qam", "second"]);

    sandbox.exec_ok("git", &["reset", "--soft", "HEAD~1"]);
    assert!(!sandbox.log().contains("git_reset"));
}

#[test]
fn reset_pins_refs_and_undo_removes_them() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.exec_ok("git", &["reset", "--hard", "HEAD"]);
    let refs = sandbox.git_ok(&["for-each-ref", "refs/rewind"]);
    assert!(refs.contains("refs/rewind/"));
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.git_ok(&["for-each-ref", "refs/rewind"]).trim(), "");
}

#[test]
fn git_clean_undo_restores_files_and_empty_directories() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("junk/sub/deep.txt", "deep");
    sandbox.write("untracked.txt", "loose");

    sandbox.exec_ok("git", &["clean", "-fd"]);
    assert!(!sandbox.exists("junk") && !sandbox.exists("untracked.txt"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("junk/sub/deep.txt"), "deep");
    assert_eq!(sandbox.read("untracked.txt"), "loose");
}

#[test]
fn git_clean_x_undo_restores_ignored_files() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write(".gitignore", "*.log\n");
    sandbox.git_ok(&["add", ".gitignore"]);
    sandbox.git_ok(&["commit", "-qm", "ignore logs"]);
    sandbox.write("debug.log", "ignored data");

    sandbox.exec_ok("git", &["clean", "-fx"]);
    assert!(!sandbox.exists("debug.log"));
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("debug.log"), "ignored data");
}

#[test]
fn git_clean_respects_exclude_patterns() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("keep.tmp", "keep");
    sandbox.write("kill.tmp", "kill");

    sandbox.exec_ok("git", &["clean", "-f", "-e", "keep.tmp"]);
    assert!(sandbox.exists("keep.tmp"));
    assert!(!sandbox.exists("kill.tmp"));
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("kill.tmp"), "kill");
}

#[test]
fn git_clean_dry_run_is_not_journaled() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("untracked", "x");
    sandbox.exec_ok("git", &["clean", "-nd"]);
    assert!(sandbox.exists("untracked"));
    assert!(!sandbox.log().contains("git_clean"));
}

#[test]
fn checkout_paths_undo_restores_worktree() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("tracked.txt", "dirty\n");

    sandbox.exec_ok("git", &["checkout", "--", "tracked.txt"]);
    assert_eq!(sandbox.read("tracked.txt"), "v1\n");

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("tracked.txt"), "dirty\n");
}

#[test]
fn checkout_tree_undo_restores_index_and_worktree() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("tracked.txt", "v2\n");
    sandbox.git_ok(&["commit", "-qam", "second"]);
    sandbox.write("tracked.txt", "v2-staged\n");
    sandbox.git_ok(&["add", "tracked.txt"]);
    sandbox.write("tracked.txt", "v2-staged\nand-unstaged\n");

    sandbox.exec_ok("git", &["checkout", "HEAD", "--", "tracked.txt"]);
    assert_eq!(sandbox.read("tracked.txt"), "v2\n");
    assert!(!sandbox.git_ok(&["status", "--porcelain"]).contains("MM"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("tracked.txt"), "v2-staged\nand-unstaged\n");
    assert!(sandbox.git_ok(&["status", "--porcelain"]).contains("MM"));
}

#[test]
fn restore_worktree_undo() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("tracked.txt", "dirty\n");

    sandbox.exec_ok("git", &["restore", "tracked.txt"]);
    assert_eq!(sandbox.read("tracked.txt"), "v1\n");
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("tracked.txt"), "dirty\n");
}

#[test]
fn restore_staged_undo() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("tracked.txt", "staged\n");
    sandbox.git_ok(&["add", "tracked.txt"]);

    sandbox.exec_ok("git", &["restore", "--staged", "tracked.txt"]);
    let status = sandbox.git_ok(&["status", "--porcelain"]);
    assert!(status.starts_with(" M"), "unexpected status: {status:?}");

    sandbox.run_ok(&["undo"]);
    let status = sandbox.git_ok(&["status", "--porcelain"]);
    assert!(status.starts_with("M "), "unexpected status: {status:?}");
}

#[test]
fn git_status_is_passed_through_without_journaling() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.exec_ok("git", &["status", "--short"]);
    assert!(sandbox.log().contains("no history") || !sandbox.log().contains("git_"));
}

#[test]
fn checkout_works_from_a_subdirectory() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("sub/file.txt", "tracked");
    sandbox.git_ok(&["add", "sub/file.txt"]);
    sandbox.git_ok(&["commit", "-qm", "sub"]);
    sandbox.write("sub/file.txt", "dirty");

    let mut command = sandbox.command();
    command
        .current_dir(sandbox.path("sub"))
        .args(["exec", "git", "--", "checkout", "--", "file.txt"]);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(sandbox.read("sub/file.txt"), "tracked");

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("sub/file.txt"), "dirty");
}

#[test]
fn git_clean_works_from_a_subdirectory() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("sub/junk.txt", "junk");

    let mut command = sandbox.command();
    command
        .current_dir(sandbox.path("sub"))
        .args(["exec", "git", "--", "clean", "-fd"]);
    assert!(command.output().unwrap().status.success());
    assert!(!sandbox.exists("sub/junk.txt"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("sub/junk.txt"), "junk");
}
