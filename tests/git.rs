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

#[test]
fn git_clean_undo_restores_a_single_empty_directory() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.mkdir("empty");

    sandbox.exec_ok("git", &["clean", "-fd"]);
    assert!(!sandbox.exists("empty"));

    sandbox.run_ok(&["undo"]);
    assert!(sandbox.path("empty").is_dir());
    assert_eq!(std::fs::read_dir(sandbox.path("empty")).unwrap().count(), 0);
}

#[test]
fn git_clean_undo_restores_nested_empty_directories() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.mkdir("tree/one/two/three");
    sandbox.mkdir("tree/one/four");
    sandbox.mkdir("tree/five/six");

    sandbox.exec_ok("git", &["clean", "-fd"]);
    assert!(!sandbox.exists("tree"));

    sandbox.run_ok(&["undo"]);
    for dir in ["tree/one/two/three", "tree/one/four", "tree/five/six"] {
        assert!(sandbox.path(dir).is_dir(), "missing {dir}");
    }
}

#[test]
fn git_clean_undo_restores_empty_directories_inside_file_trees() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("pkg/src/main.rs", "fn main() {}");
    sandbox.write("pkg/README", "docs");
    sandbox.mkdir("pkg/src/assets/empty");
    sandbox.mkdir("pkg/empty-sibling");

    sandbox.exec_ok("git", &["clean", "-fd"]);
    assert!(!sandbox.exists("pkg"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("pkg/src/main.rs"), "fn main() {}");
    assert_eq!(sandbox.read("pkg/README"), "docs");
    assert!(sandbox.path("pkg/src/assets/empty").is_dir());
    assert_eq!(
        std::fs::read_dir(sandbox.path("pkg/src/assets/empty"))
            .unwrap()
            .count(),
        0
    );
    assert!(sandbox.path("pkg/empty-sibling").is_dir());
}

#[test]
fn git_clean_only_ignored_restores_ignored_directories() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write(".gitignore", "build/\n*.log\n");
    sandbox.git_ok(&["add", ".gitignore"]);
    sandbox.git_ok(&["commit", "-qm", "ignore build output"]);
    sandbox.write("build/debug.log", "log");
    sandbox.mkdir("build/empty/nested");
    sandbox.write("keep.txt", "untracked");

    sandbox.exec_ok("git", &["clean", "-fdX"]);
    assert!(!sandbox.exists("build"));
    assert!(sandbox.exists("keep.txt"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("build/debug.log"), "log");
    assert!(sandbox.path("build/empty/nested").is_dir());
    assert_eq!(sandbox.read("keep.txt"), "untracked");
}

#[test]
fn git_clean_pathspec_restores_only_the_selected_directory() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("selected/nested/file.txt", "selected");
    sandbox.mkdir("selected/empty");
    sandbox.write("untouched/file.txt", "untouched");

    sandbox.exec_ok("git", &["clean", "-fd", "--", "selected"]);
    assert!(!sandbox.exists("selected"));
    assert!(sandbox.exists("untouched/file.txt"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("selected/nested/file.txt"), "selected");
    assert!(sandbox.path("selected/empty").is_dir());
    assert_eq!(sandbox.read("untouched/file.txt"), "untouched");
}

#[test]
fn git_clean_undo_keeps_recreated_paths_and_remains_resumable() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.write("conflict.txt", "original");
    sandbox.mkdir("empty");

    sandbox.exec_ok("git", &["clean", "-fd"]);
    assert!(!sandbox.exists("conflict.txt") && !sandbox.exists("empty"));

    // Recreate one path with a file and one as a non-empty directory after the
    // clean. Undo must not destroy either.
    sandbox.write("conflict.txt", "recreated");
    sandbox.write("empty/user.txt", "user data");

    let failed = sandbox.run(&["undo"]);
    assert!(!failed.status.success());
    assert_eq!(sandbox.read("conflict.txt"), "recreated");
    assert_eq!(sandbox.read("empty/user.txt"), "user data");

    // The conflicting file is still staged, so the entry stays resumable.
    assert!(sandbox.log().contains("undoable"));

    std::fs::remove_file(sandbox.path("conflict.txt")).unwrap();
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("conflict.txt"), "original");
    assert_eq!(sandbox.read("empty/user.txt"), "user data");
}

#[test]
fn git_clean_undo_reports_a_file_blocking_an_empty_directory() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.mkdir("expected-dir");

    sandbox.exec_ok("git", &["clean", "-fd"]);
    sandbox.write("expected-dir", "not a directory");

    let failed = sandbox.run(&["undo"]);
    assert!(!failed.status.success());
    assert_eq!(sandbox.read("expected-dir"), "not a directory");

    std::fs::remove_file(sandbox.path("expected-dir")).unwrap();
    sandbox.run_ok(&["undo"]);
    assert!(sandbox.path("expected-dir").is_dir());
}

#[test]
fn branch_delete_undo_restores_branch() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.git_ok(&["checkout", "-b", "feature/login"]);
    sandbox.write("login.txt", "login logic\n");
    sandbox.git_ok(&["add", "login.txt"]);
    sandbox.git_ok(&["commit", "-m", "login feature"]);
    let commit = sandbox.git_ok(&["rev-parse", "HEAD"]).trim().to_string();
    sandbox.git_ok(&["checkout", "main"]);

    sandbox.exec_ok("git", &["branch", "-D", "feature/login"]);
    assert!(!sandbox.git_ok(&["branch"]).contains("feature/login"));

    sandbox.run_ok(&["undo"]);
    assert!(sandbox.git_ok(&["branch"]).contains("feature/login"));
    let restored_commit = sandbox
        .git_ok(&["rev-parse", "refs/heads/feature/login"])
        .trim()
        .to_string();
    assert_eq!(restored_commit, commit);
}

#[test]
fn branch_delete_pinned_ref_survives_git_gc() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.git_ok(&["checkout", "-b", "unmerged-feat"]);
    sandbox.write("unique.txt", "unique to branch\n");
    sandbox.git_ok(&["add", "unique.txt"]);
    sandbox.git_ok(&["commit", "-m", "unique commit"]);
    let commit = sandbox.git_ok(&["rev-parse", "HEAD"]).trim().to_string();
    sandbox.git_ok(&["checkout", "main"]);

    sandbox.exec_ok("git", &["branch", "-D", "unmerged-feat"]);
    assert!(!sandbox.git_ok(&["branch"]).contains("unmerged-feat"));

    // Force git garbage collection to prune all unreachable objects.
    // The pinned ref under refs/rewind/* must prevent commit collection.
    sandbox.git_ok(&["gc", "--prune=now"]);

    sandbox.run_ok(&["undo"]);
    assert_eq!(
        sandbox
            .git_ok(&["rev-parse", "refs/heads/unmerged-feat"])
            .trim(),
        commit
    );
}

#[test]
fn branch_delete_multiple_branches_in_single_invocation() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.git_ok(&["branch", "feat1"]);
    sandbox.git_ok(&["branch", "feat2"]);

    sandbox.exec_ok("git", &["branch", "-D", "feat1", "feat2"]);
    let branch_list = sandbox.git_ok(&["branch"]);
    assert!(!branch_list.contains("feat1") && !branch_list.contains("feat2"));

    sandbox.run_ok(&["undo"]);
    let restored_list = sandbox.git_ok(&["branch"]);
    assert!(restored_list.contains("feat1") && restored_list.contains("feat2"));
}

#[test]
fn branch_delete_conflict_when_recreated_at_different_commit() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.git_ok(&["checkout", "-b", "conflict-branch"]);
    sandbox.write("v1.txt", "v1\n");
    sandbox.git_ok(&["add", "v1.txt"]);
    sandbox.git_ok(&["commit", "-m", "v1 commit"]);
    let original_commit = sandbox.git_ok(&["rev-parse", "HEAD"]).trim().to_string();
    sandbox.git_ok(&["checkout", "main"]);

    sandbox.exec_ok("git", &["branch", "-D", "conflict-branch"]);

    // Recreate the branch at a different commit.
    sandbox.write("v2.txt", "v2\n");
    sandbox.git_ok(&["add", "v2.txt"]);
    sandbox.git_ok(&["commit", "-m", "v2 on main"]);
    let new_commit = sandbox.git_ok(&["rev-parse", "HEAD"]).trim().to_string();
    sandbox.git_ok(&["branch", "conflict-branch"]);

    let failed = sandbox.run(&["undo"]);
    assert!(!failed.status.success());
    // The recreated branch must not be overwritten.
    assert_eq!(
        sandbox
            .git_ok(&["rev-parse", "refs/heads/conflict-branch"])
            .trim(),
        new_commit
    );
    assert!(sandbox.log().contains("undoable"));

    // Move or delete conflicting branch and retry undo.
    sandbox.git_ok(&["branch", "-D", "conflict-branch"]);
    sandbox.run_ok(&["undo"]);
    assert_eq!(
        sandbox
            .git_ok(&["rev-parse", "refs/heads/conflict-branch"])
            .trim(),
        original_commit
    );
}

#[test]
fn branch_delete_recreated_at_same_commit_is_clean() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.git_ok(&["branch", "same-commit-branch"]);
    let commit = sandbox
        .git_ok(&["rev-parse", "refs/heads/same-commit-branch"])
        .trim()
        .to_string();

    sandbox.exec_ok("git", &["branch", "-D", "same-commit-branch"]);
    // Recreate at identical commit.
    sandbox.git_ok(&["branch", "same-commit-branch", &commit]);

    sandbox.run_ok(&["undo"]);
    assert_eq!(
        sandbox
            .git_ok(&["rev-parse", "refs/heads/same-commit-branch"])
            .trim(),
        commit
    );
}

#[test]
fn branch_delete_unmerged_without_force_fails_and_is_not_journaled() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.git_ok(&["checkout", "-b", "unmerged-feat"]);
    sandbox.write("work.txt", "wip\n");
    sandbox.git_ok(&["add", "work.txt"]);
    sandbox.git_ok(&["commit", "-m", "wip"]);
    sandbox.git_ok(&["checkout", "main"]);

    let failed = sandbox.exec("git", &["branch", "-d", "unmerged-feat"]);
    assert!(!failed.status.success());
    assert!(sandbox.git_ok(&["branch"]).contains("unmerged-feat"));
    assert!(sandbox.log().contains("no history"));
}

#[test]
fn branch_delete_nonexistent_branch_fails_and_is_not_journaled() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();

    let failed = sandbox.exec("git", &["branch", "-D", "nonexistent-branch"]);
    assert!(!failed.status.success());
    assert!(sandbox.log().contains("no history"));
}

#[test]
fn branch_delete_current_branch_fails_and_is_not_journaled() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();

    let failed = sandbox.exec("git", &["branch", "-D", "main"]);
    assert!(!failed.status.success());
    assert!(sandbox.git_ok(&["branch"]).contains("main"));
    assert!(sandbox.log().contains("no history"));
}

#[test]
fn branch_delete_in_detached_head_works() {
    if !git_available() {
        return;
    }
    let sandbox = Sandbox::new();
    sandbox.init_repo();
    sandbox.git_ok(&["branch", "feat-detach"]);
    let commit = sandbox
        .git_ok(&["rev-parse", "refs/heads/feat-detach"])
        .trim()
        .to_string();

    sandbox.git_ok(&["checkout", "--detach", "HEAD"]);
    sandbox.exec_ok("git", &["branch", "-D", "feat-detach"]);
    assert!(!sandbox.git_ok(&["branch"]).contains("feat-detach"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(
        sandbox
            .git_ok(&["rev-parse", "refs/heads/feat-detach"])
            .trim(),
        commit
    );
}
