//! What the benchmarks share: Repositories to open, a store over the real
//! backend.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use criterion::Criterion;
use criterion::measurement::Measurement;
use tafit_git_gix::GixBackend;
use tafit_state::{AppStore, Loadable, Msg, StoreEvents};
use tempfile::TempDir;

/// Selects the full benchmark set; anything else runs the pull-request set.
pub const SUITE_ENV: &str = "TAFIT_PERF_SUITE";

/// The directory holding the real Repository snapshots (`git`, `bun`,
/// `chromium`) the nightly run opens; see `scripts/perf-snapshots.sh`.
pub const SNAPSHOTS_ENV: &str = "TAFIT_PERF_SNAPSHOTS";

/// The real Repositories the nightly run benchmarks, by snapshot folder name.
pub const SNAPSHOTS: [&str; 3] = ["git", "bun", "chromium"];

/// Whether this is the nightly run (`TAFIT_PERF_SUITE=nightly`).
pub fn nightly() -> bool {
    std::env::var(SUITE_ENV).is_ok_and(|suite| suite == "nightly")
}

/// The snapshot Repositories found under `$TAFIT_PERF_SNAPSHOTS`, as
/// (name, path). Panics in the nightly run if one is missing, so the run
/// can't pass without benchmarking it.
pub fn snapshots() -> Vec<(&'static str, PathBuf)> {
    let Some(root) = std::env::var_os(SNAPSHOTS_ENV).map(PathBuf::from) else {
        assert!(!nightly(), "the nightly run needs ${SNAPSHOTS_ENV}");
        return Vec::new();
    };
    SNAPSHOTS
        .iter()
        .filter_map(|&name| {
            let path = root.join(name);
            if path.is_dir() {
                Some((name, path))
            } else {
                assert!(!nightly(), "snapshot {} is missing", path.display());
                None
            }
        })
        .collect()
}

/// Criterion measuring with `measurement`, configured from the command line.
pub fn criterion<M: Measurement>(measurement: M) -> Criterion<M> {
    Criterion::default()
        .with_measurement(measurement)
        .configure_from_args()
}

/// A small Repository built with the git CLI in a temp dir, removed on drop.
pub struct Fixture {
    _root: TempDir,
    path: PathBuf,
}

impl Fixture {
    /// A Repository named `name` with `commits` commits on `main`, each adding
    /// one file.
    pub fn new(name: &str, commits: usize) -> Self {
        let root = tempfile::tempdir().expect("create temp dir");
        let path = root.path().join(name);
        std::fs::create_dir(&path).expect("create fixture dir");
        let fixture = Self { _root: root, path };
        fixture.git(&["init", "--quiet", "--initial-branch=main"]);
        for ix in 0..commits {
            std::fs::write(fixture.path.join(format!("file-{ix}.txt")), "tafit\n")
                .expect("write fixture file");
            fixture.git(&["add", "."]);
            fixture.git(&["commit", "--quiet", "-m", &format!("commit {ix}")]);
        }
        fixture
    }

    /// The work tree to open.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Runs git in the fixture, isolated from the user's and system's config.
    fn git(&self, args: &[&str]) {
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        let output = Command::new("git")
            .current_dir(&self.path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", null)
            .args([
                "-c",
                "user.name=tafit perf",
                "-c",
                "user.email=perf@tafit.invalid",
            ])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// A store over the real backend, as the app builds it. No UI poller runs:
/// the benchmark holds the change events itself.
pub struct Store {
    store: AppStore,
    events: StoreEvents,
}

impl Store {
    /// An empty store, as at startup.
    pub fn new() -> Self {
        let (store, events) = AppStore::new(Arc::new(GixBackend::default()));
        Self { store, events }
    }

    /// Opens `path` in a new Repository Tab and blocks until it has loaded.
    /// Panics if it fails to open, so a broken benchmark can't look fast.
    pub fn open(&self, path: &Path) {
        let tabs = self.store.snapshot().repos.len();
        self.store.dispatch(Msg::OpenRepo(path.to_owned()));
        loop {
            // `dispatch` returns before the store has reduced the message, so
            // wait for the new Repository Tab to appear, then to load.
            let state = self.store.snapshot();
            if let Some(repo) = state.repos.get(tabs) {
                match &repo.open {
                    Loadable::Loading => {}
                    Loadable::Ready(_) => return,
                    Loadable::Error(err) => panic!("{} failed to open: {err}", path.display()),
                }
            }
            // Every publication sends an event (or finds one pending), so the
            // snapshot is re-read after the last change.
            self.events.recv_blocking().expect("the store is running");
        }
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}
