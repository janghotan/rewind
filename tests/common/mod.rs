//! Shared harness for integration tests.
//!
//! Every test gets a private `$HOME`, so journals, trash, and configuration
//! cannot leak between tests even when cargo runs them in parallel.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

pub struct Sandbox {
    _root: TempDir,
    pub home: PathBuf,
    pub work: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let home = root.path().join("home");
        let work = root.path().join("work");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::create_dir_all(&work).expect("work");
        Self {
            _root: root,
            home,
            work,
        }
    }

    pub fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rewind"));
        command
            .current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("TZ", "UTC")
            .env_remove("REWIND_DATA_DIR")
            .env_remove("REWIND_JOURNAL")
            .env_remove("REWIND_CONFIG_PATH")
            .env_remove("REWIND_DISABLE");
        command
    }

    pub fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().expect("running rewind")
    }

    pub fn run_ok(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "rewind {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    pub fn run_fail(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            !output.status.success(),
            "rewind {args:?} unexpectedly succeeded\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    pub fn exec(&self, command: &str, args: &[&str]) -> Output {
        let mut full = vec!["exec", command, "--"];
        full.extend_from_slice(args);
        self.run(&full)
    }

    pub fn exec_ok(&self, command: &str, args: &[&str]) -> Output {
        let output = self.exec(command, args);
        assert!(
            output.status.success(),
            "rewind exec {command} {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    pub fn undo(&self) -> Output {
        self.run(&["undo"])
    }

    pub fn undo_n(&self, n: usize) -> Output {
        self.run(&["undo", &n.to_string()])
    }

    pub fn log(&self) -> String {
        String::from_utf8_lossy(&self.run_ok(&["log"]).stdout).to_string()
    }

    pub fn status(&self) -> String {
        String::from_utf8_lossy(&self.run_ok(&["status"]).stdout).to_string()
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.work.join(relative)
    }

    pub fn write(&self, relative: &str, contents: &str) -> PathBuf {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(&path, contents).expect("write");
        path
    }

    pub fn mkdir(&self, relative: &str) -> PathBuf {
        let path = self.path(relative);
        std::fs::create_dir_all(&path).expect("mkdir");
        path
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path(relative)).expect("read")
    }

    pub fn exists(&self, relative: &str) -> bool {
        self.path(relative).symlink_metadata().is_ok()
    }

    #[cfg(unix)]
    pub fn mode(&self, relative: &str) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(self.path(relative))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o7777
    }

    #[cfg(unix)]
    pub fn set_mode(&self, relative: &str, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(self.path(relative), std::fs::Permissions::from_mode(mode))
            .expect("chmod");
    }

    pub fn git(&self, args: &[&str]) -> Output {
        Command::new("git")
            .args([
                "-c",
                "user.name=Rewind Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "advice.detachedHead=false",
            ])
            .args(args)
            .current_dir(&self.work)
            .env("HOME", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("running git")
    }

    pub fn git_ok(&self, args: &[&str]) -> String {
        let output = self.git(args);
        assert!(
            output.status.success(),
            "git {args:?} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    pub fn git_fail(&self, args: &[&str]) -> Output {
        let output = self.git(args);
        assert!(
            !output.status.success(),
            "git {args:?} unexpectedly succeeded"
        );
        output
    }

    /// Repository with one committed file (`tracked.txt` = "v1").
    pub fn init_repo(&self) {
        self.git_ok(&["init", "-q"]);
        self.write("tracked.txt", "v1\n");
        self.git_ok(&["add", "."]);
        self.git_ok(&["commit", "-qm", "initial"]);
    }

    pub fn journal_path(&self) -> PathBuf {
        self.home.join(".local/share/rewind/journal.db")
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.home.join(".local/share/rewind/trash")
    }

    /// Run a script in bash/zsh/fish with rewind on PATH.
    pub fn run_shell(&self, shell: &str, script: &str) -> Output {
        let binary = Path::new(env!("CARGO_BIN_EXE_rewind"));
        let binary_dir = binary.parent().expect("binary dir");
        let path = format!(
            "{}:{}",
            binary_dir.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut command = Command::new(shell);
        command
            .arg("-c")
            .arg(script)
            .current_dir(&self.work)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join(".local/share"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("PATH", path)
            .env("TZ", "UTC")
            .env("LC_ALL", "C")
            .env_remove("REWIND_DATA_DIR")
            .env_remove("REWIND_JOURNAL")
            .env_remove("REWIND_CONFIG_PATH")
            .env_remove("REWIND_DISABLE");
        command.output().expect("running shell")
    }
}

pub fn have_command(name: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {name} >/dev/null 2>&1")])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Strip a trailing newline for exact message comparisons.
pub fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr)
        .trim_end()
        .to_string()
}

pub fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .trim_end()
        .to_string()
}

#[cfg(unix)]
pub fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}
