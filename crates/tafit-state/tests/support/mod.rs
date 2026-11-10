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
use tafit_state::{AppState, AppStore, Msg, StoreEvents, WatchOptions};
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
    /// `git init` with `main` as the initial branch and no commits. The
    /// store's git reads the user's config, so the repo's own config sets an
    /// identity, turns signing off, keeps hooks in `.git/hooks` and line
    /// endings as written (git for Windows turns `core.autocrlf` on).
    pub fn init(name: &str) -> Self {
        let repo = Self {
            dir: TestDir::new(name),
        };
        repo.git(&["init", "--quiet", "--initial-branch=main"]);
        for (key, value) in [
            ("user.name", "tafit test"),
            ("user.email", "test@tafit.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", ".git/hooks"),
            ("core.autocrlf", "false"),
        ] {
            repo.git(&["config", key, value]);
        }
        repo
    }

    /// Installs a shell-script hook; git for Windows runs it with its own sh.
    pub fn hook(&self, name: &str, script: &str) {
        let path = self.path().join(".git/hooks").join(name);
        std::fs::create_dir_all(path.parent().expect("hooks folder")).expect("create hooks folder");
        std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).expect("write hook");
        make_executable(&path);
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

    /// An empty commit authored and committed at `time` (seconds since the
    /// epoch, UTC), so commits made within one second still have an order.
    pub fn commit_at(&self, message: &str, time: i64) {
        let date = format!("@{time} +0000");
        let output = self
            .command(&["commit", "--quiet", "--allow-empty", "-m", message])
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git commit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Adds `count` empty commits on top of `main` in one `git fast-import`,
    /// far faster than one `git commit` each. Commit `ix` (0-based, oldest
    /// first) has the message `commit {ix}` and is a second newer than the
    /// one before it.
    pub fn import_commits(&self, count: usize) {
        use std::io::Write;
        let parent = self.run_git(&["rev-parse", "--verify", "--quiet", "main"]);
        let parent = String::from_utf8_lossy(&parent.stdout).trim().to_owned();
        let mut stream = String::new();
        for ix in 0..count {
            let time = 1_700_000_000 + ix;
            let message = format!("commit {ix}");
            stream += "commit refs/heads/main\n";
            stream += &format!("author tafit test <test@tafit.invalid> {time} +0000\n");
            stream += &format!("committer tafit test <test@tafit.invalid> {time} +0000\n");
            stream += &format!("data {}\n{message}\n", message.len());
            if ix == 0 && !parent.is_empty() {
                stream += &format!("from {parent}\n");
            }
            stream += "\n";
        }
        let mut child = self
            .command(&["fast-import", "--quiet"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .expect("run git fast-import");
        child
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(stream.as_bytes())
            .expect("write the import stream");
        assert!(
            child.wait().expect("wait for git").success(),
            "git fast-import failed"
        );
        // fast-import moves the branch but leaves the work tree as it was.
        self.git(&["reset", "--quiet", "--hard"]);
    }

    /// Writes `contents` to the repo-relative `path`, creating its folders.
    pub fn write(&self, path: &str, contents: &str) {
        let path = self.path().join(path);
        std::fs::create_dir_all(path.parent().expect("a file path has a parent"))
            .expect("create parent folders");
        std::fs::write(path, contents).expect("write test file");
    }

    /// Writes `contents` to the repo-relative `path` byte for byte, e.g.
    /// with CRLF line endings.
    pub fn write_bytes(&self, path: &str, contents: &[u8]) {
        std::fs::write(self.path().join(path), contents).expect("write test file");
    }

    /// The work tree's `path`, byte for byte.
    pub fn work_file(&self, path: &str) -> Vec<u8> {
        std::fs::read(self.path().join(path)).expect("read test file")
    }

    /// The index's `path`, byte for byte.
    pub fn index_file(&self, path: &str) -> Vec<u8> {
        let spec = format!(":{path}");
        let output = self.run_git(&["cat-file", "blob", &spec]);
        assert!(output.status.success(), "{path} is not in the index");
        output.stdout
    }

    /// Deletes the repo-relative file `path` from the work tree only.
    pub fn delete(&self, path: &str) {
        std::fs::remove_file(self.path().join(path)).expect("delete test file");
    }

    /// Writes and commits each `(path, contents)`.
    pub fn commit_files(&self, files: &[(&str, &str)], message: &str) {
        for (path, contents) in files {
            self.write(path, contents);
            self.git(&["add", "--", path]);
        }
        self.commit(message);
    }

    /// Runs git in the repo, isolated from the user's and system's config,
    /// and returns trimmed stdout. Panics if git fails.
    pub fn git(&self, args: &[&str]) -> String {
        let output = self.run_git(args);
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// Like `git`, for a command expected to fail (a conflicting merge);
    /// returns whether it succeeded.
    pub fn try_git(&self, args: &[&str]) -> bool {
        self.run_git(args).status.success()
    }

    fn run_git(&self, args: &[&str]) -> std::process::Output {
        self.command(args).output().expect("run git")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
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
            .args(args);
        command
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

    /// Watches Repositories with `options` instead of the app's.
    pub fn with_watch_options(options: WatchOptions) -> Self {
        let (store, events) =
            AppStore::with_watch_options(Arc::new(GixBackend::default()), options);
        Self { store, events }
    }

    pub fn dispatch(&self, msg: Msg) {
        self.store.dispatch(msg);
    }

    /// Waits until every `Msg` dispatched so far has been reduced, so a
    /// following `wait_until` can't see a snapshot from before them.
    pub fn flush(&self) {
        self.store.flush();
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
