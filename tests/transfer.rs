//! End-to-end tests for `mv`, `cp`, `chmod`, and `chown`.

mod common;

use common::*;

#[test]
fn mv_overwrite_then_undo_restores_both_sides() {
    let sandbox = Sandbox::new();
    sandbox.write("source.txt", "new content");
    sandbox.write("dest.txt", "old content");

    sandbox.exec_ok("mv", &["source.txt", "dest.txt"]);
    assert!(!sandbox.exists("source.txt"));
    assert_eq!(sandbox.read("dest.txt"), "new content");

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("source.txt"), "new content");
    assert_eq!(sandbox.read("dest.txt"), "old content");
}

#[test]
fn mv_multiple_sources_into_directory() {
    let sandbox = Sandbox::new();
    sandbox.write("one", "1");
    sandbox.write("two", "2");
    sandbox.mkdir("dest");

    sandbox.exec_ok("mv", &["one", "two", "dest"]);
    assert_eq!(sandbox.read("dest/one"), "1");
    assert_eq!(sandbox.read("dest/two"), "2");

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("one"), "1");
    assert_eq!(sandbox.read("two"), "2");
    assert!(!sandbox.exists("dest/one"));
}

#[test]
fn mv_into_directory_overwriting_an_existing_entry() {
    let sandbox = Sandbox::new();
    sandbox.write("file.txt", "incoming");
    sandbox.write("dest/file.txt", "previous");
    sandbox.mkdir("dest");

    sandbox.exec_ok("mv", &["file.txt", "dest"]);
    assert_eq!(sandbox.read("dest/file.txt"), "incoming");

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("dest/file.txt"), "previous");
    assert_eq!(sandbox.read("file.txt"), "incoming");
}

#[test]
fn mv_no_clobber_is_left_alone_and_not_journaled() {
    let sandbox = Sandbox::new();
    sandbox.write("a", "a");
    sandbox.write("b", "b");

    // GNU mv versions differ on the exit status of `-n` when it skips a file,
    // so only the observable no-clobber behavior and the absent journal entry
    // are asserted here.
    sandbox.exec("mv", &["-n", "a", "b"]);
    assert_eq!(sandbox.read("a"), "a");
    assert_eq!(sandbox.read("b"), "b");
    assert!(!sandbox.log().contains("mv -n"));
}

#[test]
fn mv_directory_then_undo() {
    let sandbox = Sandbox::new();
    sandbox.write("dir/inner.txt", "nested");
    sandbox.mkdir("target");

    sandbox.exec_ok("mv", &["dir", "target"]);
    assert_eq!(sandbox.read("target/dir/inner.txt"), "nested");

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("dir/inner.txt"), "nested");
    assert!(!sandbox.exists("target/dir"));
}

#[test]
fn cp_overwrite_then_undo_restores_destination() {
    let sandbox = Sandbox::new();
    sandbox.write("src", "replacement");
    sandbox.write("dst", "original");

    sandbox.exec_ok("cp", &["src", "dst"]);
    assert_eq!(sandbox.read("dst"), "replacement");

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.read("dst"), "original");
    assert_eq!(sandbox.read("src"), "replacement");
}

#[test]
fn cp_created_file_is_removed_on_undo() {
    let sandbox = Sandbox::new();
    sandbox.write("src", "content");

    sandbox.exec_ok("cp", &["src", "brand-new"]);
    assert!(sandbox.exists("brand-new"));

    sandbox.run_ok(&["undo"]);
    assert!(!sandbox.exists("brand-new"));
    assert_eq!(sandbox.read("src"), "content");
}

#[test]
fn cp_recursive_new_tree_is_removed_on_undo() {
    let sandbox = Sandbox::new();
    sandbox.write("src/a/b.txt", "deep");

    sandbox.exec_ok("cp", &["-r", "src", "copy"]);
    assert_eq!(sandbox.read("copy/a/b.txt"), "deep");

    sandbox.run_ok(&["undo"]);
    assert!(!sandbox.exists("copy"));
}

#[test]
fn cp_recursive_merge_undo_only_removes_the_new_tree() {
    let sandbox = Sandbox::new();
    sandbox.write("src/new.txt", "new");
    sandbox.write("dest/existing.txt", "keep");

    sandbox.exec_ok("cp", &["-r", "src", "dest"]);
    assert_eq!(sandbox.read("dest/src/new.txt"), "new");

    sandbox.run_ok(&["undo"]);
    assert!(!sandbox.exists("dest/src"));
    assert_eq!(sandbox.read("dest/existing.txt"), "keep");
}

#[test]
fn cp_target_directory_flag() {
    let sandbox = Sandbox::new();
    sandbox.write("f", "f");
    sandbox.mkdir("out");

    sandbox.exec_ok("cp", &["-t", "out", "f"]);
    assert_eq!(sandbox.read("out/f"), "f");
    sandbox.run_ok(&["undo"]);
    assert!(!sandbox.exists("out/f"));
}

#[test]
fn cp_preserves_file_modes_in_snapshot() {
    let sandbox = Sandbox::new();
    sandbox.write("src", "new");
    sandbox.write("dst", "old");
    sandbox.set_mode("dst", 0o640);

    sandbox.exec_ok("cp", &["src", "dst"]);
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.mode("dst"), 0o640);
}

#[test]
fn chmod_single_file_undo() {
    let sandbox = Sandbox::new();
    sandbox.write("run.sh", "#!/bin/sh\n");
    sandbox.set_mode("run.sh", 0o644);

    sandbox.exec_ok("chmod", &["755", "run.sh"]);
    assert_eq!(sandbox.mode("run.sh"), 0o755);
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.mode("run.sh"), 0o644);
}

#[test]
fn chmod_recursive_undo_restores_every_mode() {
    let sandbox = Sandbox::new();
    sandbox.write("tree/a", "a");
    sandbox.write("tree/sub/b", "b");
    sandbox.set_mode("tree", 0o750);
    sandbox.set_mode("tree/a", 0o600);
    sandbox.set_mode("tree/sub", 0o711);
    sandbox.set_mode("tree/sub/b", 0o640);

    sandbox.exec_ok("chmod", &["-R", "777", "tree"]);
    assert_eq!(sandbox.mode("tree/a"), 0o777);
    assert_eq!(sandbox.mode("tree/sub/b"), 0o777);

    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.mode("tree"), 0o750);
    assert_eq!(sandbox.mode("tree/a"), 0o600);
    assert_eq!(sandbox.mode("tree/sub"), 0o711);
    assert_eq!(sandbox.mode("tree/sub/b"), 0o640);
}

#[test]
fn chmod_symbolic_mode_undo() {
    let sandbox = Sandbox::new();
    sandbox.write("script", "x");
    sandbox.set_mode("script", 0o644);
    sandbox.exec_ok("chmod", &["u+x", "script"]);
    assert_eq!(sandbox.mode("script"), 0o744);
    sandbox.run_ok(&["undo"]);
    assert_eq!(sandbox.mode("script"), 0o644);
}

#[cfg(unix)]
#[test]
fn chown_to_self_is_recorded_and_undo_succeeds() {
    use std::process::Command;
    let sandbox = Sandbox::new();
    sandbox.write("file", "x");
    let uid = String::from_utf8_lossy(&Command::new("id").arg("-u").output().expect("id").stdout)
        .trim()
        .to_string();
    let gid = String::from_utf8_lossy(&Command::new("id").arg("-g").output().expect("id").stdout)
        .trim()
        .to_string();
    let owner = format!("{uid}:{gid}");
    let output = sandbox.exec("chown", &[&owner, "file"]);
    if !output.status.success() {
        // Running as a user without chown permission: nothing to test.
        return;
    }
    assert!(sandbox.log().contains("chown"));
    sandbox.run_ok(&["undo"]);
}

#[test]
fn disabled_wrapper_is_a_true_passthrough() {
    let sandbox = Sandbox::new();
    sandbox.run_ok(&["config", "set", "wrap.cp", "false"]);
    sandbox.write("src", "new");
    sandbox.write("dst", "old");

    sandbox.exec_ok("cp", &["src", "dst"]);
    assert_eq!(sandbox.read("dst"), "new");
    assert!(!sandbox.log().contains("cp src dst"));
    let output = sandbox.run_ok(&["undo"]);
    assert!(stdout_of(&output).contains("nothing to undo"));
}

#[test]
fn mv_failed_operation_keeps_destination_and_does_not_create_an_entry() {
    let sandbox = Sandbox::new();
    sandbox.write("dest", "keep");
    let output = sandbox.exec("mv", &["missing", "dest"]);
    assert!(!output.status.success());
    assert_eq!(sandbox.read("dest"), "keep");
    assert!(!sandbox.log().contains("mv missing dest"));
}
