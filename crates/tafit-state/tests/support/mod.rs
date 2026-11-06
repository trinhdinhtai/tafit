//! Test harness for the main seam: a headless store over real Git repos.
//!
//! Each test builds a throwaway repo with the git CLI, sends `Msg`s to an
//! `AppStore`, waits for the effects to report back, then checks the snapshot.
//! No UI poller runs: the test holds the store's event receiver itself.

#![allow(dead_code)] // Each test binary uses a different subset.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tafit_git_gix::GixBackend;
use tafit_state::{AppState, AppStore, Msg, StoreEvents};
use tempfile::TempDir;

const WAIT_TIMEOUT: Duration = Duration::from_secs(10);

/// A directory inside a fresh temp dir, removed when dropped.
pub struct TestDir {
    _root: TempDir,
    pub path: PathBuf,
}

impl TestDir {
    /// An empty, non-repo directory named `name`.
    pub fn new(name: &str) -> Self {
        let root = tempfile::tempdir().expect("create temp dir");
        let path = root.path().join(name);
        std::fs::create_dir(&path).expect("create test dir");
        Self { _root: root, path }
    }
}

/// A real Git repository built with the git CLI.
pub struct TestRepo {
    dir: TestDir,
}

impl TestRepo {
    /// `git init` with `main` as the initial branch and no commits.
    pub fn init(name: &str) -> Self {
        let repo = Self {
            dir: TestDir::new(name),
        };
        repo.git(&["init", "--quiet", "--initial-branch=main"]);
        repo
    }

    /// `git init --bare` with `main` as the initial branch.
    pub fn init_bare(name: &str) -> Self {
        let repo = Self {
            dir: TestDir::new(name),
        };
        repo.git(&["init", "--quiet", "--bare", "--initial-branch=main"]);
        repo
    }

    /// Like `init`, plus one empty commit on `main`.
    pub fn with_commit(name: &str) -> Self {
        let repo = Self::init(name);
        repo.commit("initial");
        repo
    }

    pub fn path(&self) -> &Path {
        &self.dir.path
    }

    pub fn commit(&self, message: &str) {
        self.git(&["commit", "--quiet", "--allow-empty", "-m", message]);
    }

    /// Runs git in the repo, isolated from the user's and system's config,
    /// and returns trimmed stdout. Panics if git fails.
    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(self.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null_device())
            .args([
                "-c",
                "user.name=tafit test",
                "-c",
                "user.email=test@tafit.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
}

/// A script standing in for git: a batch file on Windows, a shell script
/// elsewhere. It ignores its arguments.
pub struct FakeGit {
    dir: TestDir,
    program: PathBuf,
}

impl FakeGit {
    /// Prints `stdout` and `stderr` (one line each, skipped when empty), then
    /// exits with `exit_code`.
    pub fn printing(stdout: &str, stderr: &str, exit_code: i32) -> Self {
        let (mut windows, mut unix) = (String::from("@echo off\r\n"), String::new());
        if !stdout.is_empty() {
            windows += &format!("echo {stdout}\r\n");
            unix += &format!("printf '%s\\n' '{stdout}'\n");
        }
        if !stderr.is_empty() {
            windows += &format!("echo {stderr} 1>&2\r\n");
            unix += &format!("printf '%s\\n' '{stderr}' >&2\n");
        }
        windows += &format!("exit /b {exit_code}\r\n");
        unix += &format!("exit {exit_code}\n");
        Self::new(&windows, &unix)
    }

    /// A git 2.53.0 whose `git config ...` prints `config_lines`.
    pub fn with_config(config_lines: &[String]) -> Self {
        let (mut windows, mut unix) = (
            String::from("@echo off\r\nif not \"%1\"==\"config\" goto version\r\n"),
            String::from("if [ \"$1\" = config ]; then\n"),
        );
        for line in config_lines {
            windows += &format!("echo {line}\r\n");
            unix += &format!("printf '%s\\n' '{line}'\n");
        }
        windows += "exit /b 0\r\n:version\r\necho git version 2.53.0\r\n";
        unix += "exit 0\nfi\necho 'git version 2.53.0'\n";
        Self::new(&windows, &unix)
    }

    /// Starts a child process that waits about two seconds and marks
    /// `finished`, marks `started`, then waits. Only killing the whole process
    /// tree keeps `finished` from appearing. On Windows the child gets its own
    /// console: one sharing git's would die with git anyway.
    pub fn hanging() -> Self {
        Self::new(
            "@echo off\r\n\
             start \"\" /min cmd /c \"ping -n 3 127.0.0.1 >nul & echo.>\"%~dp0finished\"\"\r\n\
             echo.>\"%~dp0started\"\r\n\
             ping -n 4 127.0.0.1 >nul\r\n",
            "dir=$(dirname \"$0\")\n\
             (sleep 2; : > \"$dir/finished\") &\n\
             : > \"$dir/started\"\n\
             wait\n",
        )
    }

    fn new(windows: &str, unix: &str) -> Self {
        let dir = TestDir::new("fake-git");
        let program = if cfg!(windows) {
            let program = dir.path.join("git.cmd");
            std::fs::write(&program, windows).expect("write fake git");
            program
        } else {
            let program = dir.path.join("git");
            std::fs::write(&program, format!("#!/bin/sh\n{unix}")).expect("write fake git");
            make_executable(&program);
            program
        };
        Self { dir, program }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Waits until a `hanging` fake has started.
    pub fn wait_until_started(&self) {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        while !self.dir.path.join("started").exists() {
            assert!(Instant::now() < deadline, "the fake git never started");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Whether a `hanging` fake ran to its end.
    pub fn finished(&self) -> bool {
        self.dir.path.join("finished").exists()
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("make fake git executable");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// An `AppStore` backed by gix, with the test holding its event receiver.
pub struct TestStore {
    pub store: AppStore,
    events: StoreEvents,
}

impl TestStore {
    /// Runs the git on `PATH`.
    pub fn new() -> Self {
        Self::with_backend(GixBackend::default())
    }

    /// Runs the git at `program` instead of the one on `PATH`.
    pub fn with_git(program: &Path) -> Self {
        Self::with_backend(GixBackend::with_git(program))
    }

    fn with_backend(backend: GixBackend) -> Self {
        let (store, events) = AppStore::new(Arc::new(backend));
        Self { store, events }
    }

    pub fn dispatch(&self, msg: Msg) {
        self.store.dispatch(msg);
    }

    /// Waits until `done` holds for a published snapshot, then returns it.
    /// Panics with the last snapshot if that takes longer than `WAIT_TIMEOUT`.
    pub fn wait_until(&self, done: impl Fn(&AppState) -> bool) -> Arc<AppState> {
        let deadline = Instant::now() + WAIT_TIMEOUT;
        loop {
            let snapshot = self.store.snapshot();
            if done(&snapshot) {
                return snapshot;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the store; last snapshot: {snapshot:#?}"
            );
            // Drain coalesced change events; the snapshot is the source of truth.
            while self.events.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
