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

fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// An `AppStore` backed by gix, with the test holding its event receiver.
pub struct TestStore {
    pub store: AppStore,
    events: StoreEvents,
}

impl TestStore {
    pub fn new() -> Self {
        let (store, events) = AppStore::new(Arc::new(GixBackend));
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
