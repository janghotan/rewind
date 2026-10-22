//! End-to-end tests for `rm` interception and undo.

mod common;

use std::fs;

use common::*;

#[test]
fn rm_file_then_undo_restores_it() {
    let sandbox = Sandbox::new();
    sandbox.write("notes.txt", "important data\n");

    let output = sandbox.exec_ok("rm", &["notes.txt"]);
    assert_eq!(stdout_of(&output), "");
    assert_eq!(stderr_of(&output), "");
    assert!(!sandbox.exists("notes.txt"));
    assert!(sandbox.log().contains("rm notes.txt"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("notes.txt"), "important data\n");
    assert!(!sandbox.log().contains("undoable"));
}

#[test]
fn rm_tree_preserves_nested_content_and_symlinks() {
    let sandbox = Sandbox::new();
    sandbox.write("project/src/main.rs", "fn main() {}\n");
    sandbox.write("project/README.md", "# hi\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink("src/main.rs", sandbox.path("project/link")).unwrap();

    sandbox.exec_ok("rm", &["-rf", "project"]);
    assert!(!sandbox.exists("project"));
    sandbox.run_ok(&["undo"]);

    assert_eq!(sandbox.read("project/src/main.rs"), "fn main() {}\n");
    #[cfg(unix)]
    {
        let link = sandbox.path("project/link");
        assert!(is_symlink(&link));
        assert_eq!(
            fs::read_link(link).unwrap().display().to_string(),
            "src/main.rs"
        );
    }
}

#[test]
fn rm_multiple_files_and_directory_in_one_call() {
    let sandbox = Sandbox::new();
    sandbox.write("a.txt", "a");
    sandbox.write("b.txt", "b");
    sandbox.write("dir/c.txt", "c");

    sandbox.run_ok(&["exec", "rm", "--", "-r", "a.txt", "b.txt", "dir"]);
    assert!(!sandbox.exists("a.txt") && !sandbox.exists("dir"));
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("a.txt"), "a");
    assert_eq!(sandbox.read("b.txt"), "b");
    assert_eq!(sandbox.read("dir/c.txt"), "c");
}

#[test]
fn partial_failure_still_undoes_what_was_removed() {
    let sandbox = Sandbox::new();
    sandbox.write("kept.txt", "data");

    let output = sandbox.run_fail(&["exec", "rm", "--", "kept.txt", "missing.txt"]);
    assert!(stderr_of(&output).contains("missing.txt"));
    assert!(!sandbox.exists("kept.txt"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("kept.txt"), "data");
}

#[test]
fn missing_operand_matches_coreutils_message_and_exit_code() {
    let sandbox = Sandbox::new();
    let output = sandbox.run_fail(&["exec", "rm", "--", "nope.txt"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        stderr_of(&output),
        "rm: cannot remove 'nope.txt': No such file or directory"
    );
}

#[test]
fn force_missing_operand_is_a_silent_noop() {
    let sandbox = Sandbox::new();
    let output = sandbox.exec_ok("rm", &["-f", "nope.txt"]);
    assert_eq!(stdout_of(&output), "");
    assert_eq!(stderr_of(&output), "");
    assert_eq!(sandbox.log().matches("nope.txt").count(), 0);
}

#[test]
fn directory_without_recursive_is_refused() {
    let sandbox = Sandbox::new();
    sandbox.mkdir("dir");
    let output = sandbox.run_fail(&["exec", "rm", "--", "dir"]);
    assert_eq!(
        stderr_of(&output),
        "rm: cannot remove 'dir': Is a directory"
    );
    assert!(sandbox.exists("dir"));
}

#[test]
fn rm_d_removes_only_empty_directories() {
    let sandbox = Sandbox::new();
    sandbox.mkdir("empty");
    sandbox.exec_ok("rm", &["-d", "empty"]);
    assert!(!sandbox.exists("empty"));
    sandbox.run_ok(&["undo"]);
    assert!(sandbox.exists("empty"));

    sandbox.write("full/file", "x");
    let output = sandbox.run_fail(&["exec", "rm", "--", "-d", "full"]);
    assert_eq!(
        stderr_of(&output),
        "rm: cannot remove 'full': Directory not empty"
    );
}

#[test]
fn verbose_reports_removed_paths() {
    let sandbox = Sandbox::new();
    sandbox.write("v.txt", "x");
    let output = sandbox.exec_ok("rm", &["-v", "v.txt"]);
    assert_eq!(stdout_of(&output), "removed 'v.txt'");
}

#[test]
fn interactive_decline_keeps_the_file() {
    let sandbox = Sandbox::new();
    sandbox.write("i.txt", "x");
    let mut child = sandbox
        .command()
        .args(["exec", "rm", "--", "-i", "i.txt"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(b"n\n").unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(sandbox.exists("i.txt"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("remove regular file"));
}

#[test]
fn interactive_accept_removes_and_undo_restores() {
    let sandbox = Sandbox::new();
    sandbox.write("i.txt", "x");
    let mut child = sandbox
        .command()
        .args(["exec", "rm", "--", "-i", "i.txt"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        child.stdin.as_mut().unwrap().write_all(b"y\n").unwrap();
    }
    assert!(child.wait().unwrap().success());
    assert!(!sandbox.exists("i.txt"));
    sandbox.run_ok(&["undo"]);
    assert!(sandbox.exists("i.txt"));
}

#[test]
fn root_is_protected_by_default() {
    let sandbox = Sandbox::new();
    let output = sandbox.run_fail(&["exec", "rm", "--", "-rf", "/"]);
    assert!(stderr_of(&output).contains("dangerous to operate recursively on '/'"));
}

#[test]
fn refuses_to_remove_an_ancestor_of_the_trash() {
    let sandbox = Sandbox::new();
    sandbox.exec_ok("rm", &["-f", "warmup"]);
    let data_parent = sandbox.home.join(".local/share");
    let output = sandbox.run_fail(&["exec", "rm", "--", "-rf", data_parent.to_str().unwrap()]);
    assert!(stderr_of(&output).contains("refusing to stage"));
    assert!(sandbox.trash_dir().exists());
}

#[test]
fn undo_refuses_to_overwrite_a_recreated_file() {
    let sandbox = Sandbox::new();
    sandbox.write("f.txt", "old");
    sandbox.exec_ok("rm", &["f.txt"]);
    sandbox.write("f.txt", "new");

    let output = sandbox.run_fail(&["undo"]);
    assert!(stderr_of(&output).contains("refusing to overwrite"));
    assert_eq!(sandbox.read("f.txt"), "new");
    assert!(sandbox.log().contains("undoable"));

    // Moving the new file away makes the undo succeed.
    fs::rename(sandbox.path("f.txt"), sandbox.path("f.txt.bak")).unwrap();
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("f.txt"), "old");
}

#[test]
fn modes_and_mtimes_survive_undo() {
    let sandbox = Sandbox::new();
    sandbox.write("exec.sh", "#!/bin/sh\n");
    sandbox.set_mode("exec.sh", 0o751);
    let before = fs::metadata(sandbox.path("exec.sh"))
        .unwrap()
        .modified()
        .unwrap();

    sandbox.exec_ok("rm", &["exec.sh"]);
    sandbox.run_ok(&["undo"]);

    assert_eq!(sandbox.mode("exec.sh"), 0o751);
    let after = fs::metadata(sandbox.path("exec.sh"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn filenames_with_spaces_quotes_and_newlines() {
    let sandbox = Sandbox::new();
    sandbox.write("a file with spaces.txt", "spaces");
    sandbox.write("quote'file.txt", "quotes");
    sandbox.write("line\nbreak.txt", "newline");

    sandbox.exec_ok(
        "rm",
        &[
            "a file with spaces.txt",
            "quote'file.txt",
            "line\nbreak.txt",
        ],
    );
    assert!(!sandbox.exists("a file with spaces.txt"));
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("a file with spaces.txt"), "spaces");
    assert_eq!(sandbox.read("quote'file.txt"), "quotes");
    assert_eq!(sandbox.read("line\nbreak.txt"), "newline");
}

#[test]
fn symlink_removal_does_not_touch_the_target() {
    let sandbox = Sandbox::new();
    sandbox.write("target.txt", "target");
    #[cfg(unix)]
    std::os::unix::fs::symlink("target.txt", sandbox.path("link")).unwrap();

    sandbox.exec_ok("rm", &["link"]);
    assert!(sandbox.exists("target.txt"));
    assert!(!sandbox.exists("link"));
    sandbox.run_ok(&["undo"]);
    #[cfg(unix)]
    assert!(is_symlink(&sandbox.path("link")));
}

#[test]
fn undo_n_reverts_multiple_commands_newest_first() {
    let sandbox = Sandbox::new();
    sandbox.write("one", "1");
    sandbox.write("two", "2");
    sandbox.write("three", "3");

    sandbox.exec_ok("rm", &["one"]);
    sandbox.exec_ok("rm", &["two"]);
    sandbox.exec_ok("rm", &["three"]);

    let output = sandbox.run(&["undo", "2"]);
    assert!(output.status.success());
    // Only the oldest removal stays applied.
    assert!(!sandbox.exists("one"));
    assert!(sandbox.exists("two"));
    assert!(sandbox.exists("three"));

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("one"), "1");
}

#[test]
fn dry_run_undo_changes_nothing() {
    let sandbox = Sandbox::new();
    sandbox.write("f", "x");
    sandbox.exec_ok("rm", &["f"]);
    let output = sandbox.run_ok(&["undo", "--dry-run"]);
    assert!(stdout_of(&output).contains("would undo"));
    assert!(!sandbox.exists("f"));
    assert!(sandbox.log().contains("undoable"));
}

#[test]
fn undo_with_empty_journal_is_friendly() {
    let sandbox = Sandbox::new();
    let output = sandbox.run_ok(&["undo"]);
    assert!(stdout_of(&output).contains("nothing to undo"));
}

#[cfg(unix)]
#[test]
fn data_directory_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    sandbox.write("f", "x");
    sandbox.exec_ok("rm", &["f"]);
    let mode = fs::metadata(sandbox.trash_dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}
