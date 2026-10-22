//! CLI surface tests: help, config, log/status JSON, clean, completions.

mod common;

use common::*;

#[test]
fn help_and_version_work() {
    let sandbox = Sandbox::new();
    let help = sandbox.run_ok(&["--help"]);
    let text = String::from_utf8_lossy(&help.stdout);
    for command in ["undo", "log", "clean", "status", "init", "config"] {
        assert!(text.contains(command), "help missing {command}");
    }
    let version = sandbox.run_ok(&["--version"]);
    assert!(stdout_of(&version).starts_with("rewind "));
}

#[test]
fn shell_init_scripts_are_emitted() {
    let sandbox = Sandbox::new();
    for shell in ["bash", "zsh", "fish"] {
        let output = sandbox.run_ok(&["init", shell]);
        let script = String::from_utf8_lossy(&output.stdout);
        assert!(script.contains("rewind exec"), "{shell} hook incomplete");
        assert!(
            script.contains("REWIND_DISABLE"),
            "{shell} hook lacks bypass"
        );
    }
}

#[test]
fn completions_are_generated() {
    let sandbox = Sandbox::new();
    let output = sandbox.run_ok(&["completions", "bash"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("_rewind"));
}

#[test]
fn status_and_log_json_are_valid() {
    let sandbox = Sandbox::new();
    sandbox.write("f", "x");
    sandbox.exec_ok("rm", &["f"]);

    let status: serde_json::Value =
        serde_json::from_slice(&sandbox.run_ok(&["status", "--json"]).stdout).unwrap();
    assert_eq!(status["undoable"], 1);
    assert!(status["data_dir"].as_str().unwrap().contains("rewind"));

    let log: serde_json::Value =
        serde_json::from_slice(&sandbox.run_ok(&["log", "--json"]).stdout).unwrap();
    assert_eq!(log[0]["command"], "rm f");
    assert_eq!(log[0]["undoable"], true);
    assert_eq!(log[0]["status"], "done");
}

#[test]
fn config_path_show_get_set_roundtrip() {
    let sandbox = Sandbox::new();
    let path = stdout_of(&sandbox.run_ok(&["config", "path"]));
    assert!(path.ends_with("config.toml"));

    let shown = stdout_of(&sandbox.run_ok(&["config", "show"]));
    assert!(shown.contains("[wrap]"));
    assert!(shown.contains("rm = true"));

    assert_eq!(
        stdout_of(&sandbox.run_ok(&["config", "get", "wrap.rm"])),
        "true"
    );
    sandbox.run_ok(&["config", "set", "wrap.rm", "false"]);
    assert_eq!(
        stdout_of(&sandbox.run_ok(&["config", "get", "wrap.rm"])),
        "false"
    );
    assert_eq!(
        stdout_of(&sandbox.run_ok(&["config", "get", "trash.retention_days"])),
        "30"
    );

    let output = sandbox.run_fail(&["config", "set", "wrap.nope", "true"]);
    assert!(stderr_of(&output).contains("unknown config key"));
}

#[test]
fn log_reports_empty_journal() {
    let sandbox = Sandbox::new();
    assert!(stdout_of(&sandbox.run_ok(&["log"])).contains("no history"));
}

#[test]
fn log_limit_is_respected() {
    let sandbox = Sandbox::new();
    for name in ["a", "b", "c"] {
        sandbox.write(name, name);
        sandbox.exec_ok("rm", &[name]);
    }
    let log = String::from_utf8_lossy(&sandbox.run_ok(&["log", "-n", "2"]).stdout).to_string();
    assert!(log.contains("rm c") && log.contains("rm b"));
    assert!(!log.contains("rm a"));
}

#[test]
fn clean_requires_a_policy_when_retention_is_disabled() {
    let sandbox = Sandbox::new();
    sandbox.run_ok(&["config", "set", "trash.retention_days", "0"]);
    sandbox.write("f", "x");
    sandbox.exec_ok("rm", &["f"]);

    let output = sandbox.run_fail(&["clean"]);
    assert!(stderr_of(&output).contains("retention is disabled"));

    sandbox.run_ok(&["clean", "--older-than", "0d"]);
    assert!(sandbox.log().contains("expired"));
    let undo = sandbox.run_ok(&["undo"]);
    assert!(stdout_of(&undo).contains("nothing to undo"));
}

#[test]
fn clean_dry_run_does_not_purge() {
    let sandbox = Sandbox::new();
    sandbox.write("f", "x");
    sandbox.exec_ok("rm", &["f"]);

    let output = sandbox.run_ok(&["clean", "--all", "--dry-run"]);
    assert!(stdout_of(&output).contains("would purge 1 entry"));
    assert!(sandbox.log().contains("undoable"));

    sandbox.run_ok(&["clean", "--all"]);
    assert!(sandbox.log().contains("expired"));
}

#[test]
fn clean_prunes_empty_trash_and_keeps_journal_history() {
    let sandbox = Sandbox::new();
    sandbox.write("f", "x");
    sandbox.exec_ok("rm", &["f"]);
    sandbox.run_ok(&["clean", "--all"]);
    let entries = std::fs::read_dir(sandbox.trash_dir()).unwrap().count();
    assert_eq!(entries, 0);
    assert!(sandbox.log().contains("rm f"));
}

#[test]
fn undo_zero_is_rejected() {
    let sandbox = Sandbox::new();
    let output = sandbox.run_fail(&["undo", "0"]);
    assert!(stderr_of(&output).contains("at least 1"));
}

#[test]
fn status_reports_enabled_wrappers() {
    let sandbox = Sandbox::new();
    let status = sandbox.status();
    assert!(status.contains("rm mv cp chmod chown git"));
    sandbox.run_ok(&["config", "set", "wrap.mv", "false"]);
    let status = sandbox.status();
    assert!(!status.contains("rm mv "));
}

#[test]
fn no_arguments_shows_help() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&[]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Usage:"));
}

#[test]
fn config_init_creates_a_file_once() {
    let sandbox = Sandbox::new();
    let first = stdout_of(&sandbox.run_ok(&["config", "init"]));
    assert!(std::path::Path::new(&first).exists());
    let second = stdout_of(&sandbox.run_ok(&["config", "init"]));
    assert_eq!(first, second);
    assert!(sandbox.run_ok(&["status"]).status.success());
}
