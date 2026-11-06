//! What the benchmarks share: Repositories to open, a store over the real
//! backend, and measurements for idle CPU and memory.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use criterion::measurement::{Measurement, ValueFormatter};
use criterion::{Criterion, Throughput};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tafit_core::RepositoryStatus;
use tafit_git_gix::GixBackend;
use tafit_state::{AppStore, Loadable, Msg, RepoId, RepoState, StoreEvents};
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

    /// A Repository named `name` with `files` committed files spread over
    /// folders of 200, then a typical day's work on top: 1% of them edited,
    /// a tenth of those staged, and 200 new untracked files.
    pub fn large(name: &str, files: usize) -> Self {
        let fixture = Self::new(name, 0);
        let file = |ix: usize| format!("dir-{}/file-{ix}.txt", ix / 200);
        for ix in 0..files {
            let path = fixture.path.join(file(ix));
            if ix % 200 == 0 {
                std::fs::create_dir_all(path.parent().expect("a file has a folder"))
                    .expect("create fixture folder");
            }
            std::fs::write(path, format!("tafit {ix}\n")).expect("write fixture file");
        }
        // Files written in the same second as the index are "racily clean", and
        // every status would re-hash them; a real index rarely has many.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "--quiet", "-m", "files"]);
        let edited: Vec<_> = (0..files).step_by(100).map(file).collect();
        for path in &edited {
            std::fs::write(fixture.path.join(path), "edited\n").expect("edit fixture file");
        }
        let mut stage = vec!["add", "--"];
        stage.extend(edited.iter().step_by(10).map(String::as_str));
        fixture.git(&stage);
        // At the top level, so each is listed rather than collapsed into a folder.
        for ix in 0..200 {
            std::fs::write(fixture.path.join(format!("new-{ix}.txt")), "new\n")
                .expect("write untracked file");
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

    /// Opens `path` like `open`, then also waits for its first change list.
    pub fn open_with_status(&self, path: &Path) -> RepoId {
        let repo_id = self.open(path);
        self.wait_for_status(repo_id, None);
        repo_id
    }

    /// Re-reads `repo_id`'s change list (`Msg::RefreshStatus`) and blocks
    /// until the new one is published.
    pub fn refresh_status(&self, repo_id: RepoId) {
        let previous = match &self.repo(repo_id).status {
            Loadable::Ready(status) => Arc::clone(status),
            _ => panic!("refresh_status needs a change list to replace"),
        };
        self.store.dispatch(Msg::RefreshStatus(repo_id));
        self.wait_for_status(repo_id, Some(&previous));
    }

    /// Blocks until `repo_id` has a change list other than `previous`. Panics
    /// if it fails to load.
    fn wait_for_status(&self, repo_id: RepoId, previous: Option<&Arc<RepositoryStatus>>) {
        loop {
            match &self.repo(repo_id).status {
                Loadable::Ready(status) if previous.is_none_or(|p| !Arc::ptr_eq(p, status)) => {
                    return;
                }
                Loadable::Error(err) => panic!("status failed: {err}"),
                _ => {}
            }
            self.events.recv_blocking().expect("the store is running");
        }
    }

    fn repo(&self, repo_id: RepoId) -> RepoState {
        let state = self.store.snapshot();
        state
            .repos
            .iter()
            .find(|repo| repo.id == repo_id)
            .expect("the Repository is open")
            .clone()
    }

    /// Opens `path` in a new Repository Tab and blocks until it has loaded.
    /// Panics if it fails to open, so a broken benchmark can't look fast.
    pub fn open(&self, path: &Path) -> RepoId {
        let tabs = self.store.snapshot().repos.len();
        self.store.dispatch(Msg::OpenRepo(path.to_owned()));
        loop {
            // `dispatch` returns before the store has reduced the message, so
            // wait for the new Repository Tab to appear, then to load.
            let state = self.store.snapshot();
            if let Some(repo) = state.repos.get(tabs) {
                match &repo.open {
                    Loadable::Loading => {}
                    Loadable::Ready(_) => return repo.id,
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

/// Reads this process's counters through `sysinfo`. Keeps one `System` so a
/// reading doesn't allocate, which would show up as memory growth.
struct ThisProcess {
    pid: Pid,
    system: RefCell<System>,
}

impl ThisProcess {
    fn new() -> Self {
        Self {
            pid: sysinfo::get_current_pid().expect("this process's pid"),
            system: RefCell::new(System::new().expect("read system information")),
        }
    }

    fn read<T>(&self, kind: ProcessRefreshKind, value: impl Fn(&sysinfo::Process) -> T) -> T {
        let mut system = self.system.borrow_mut();
        system.refresh_processes_specifics(ProcessesToUpdate::Some(&[self.pid]), false, kind);
        value(system.process(self.pid).expect("this process is listed"))
    }
}

/// CPU time this process used, in nanoseconds (millisecond resolution).
pub struct CpuTime(ThisProcess);

impl Default for CpuTime {
    fn default() -> Self {
        Self(ThisProcess::new())
    }
}

impl CpuTime {
    fn now(&self) -> u64 {
        let millis = self.0.read(ProcessRefreshKind::nothing().with_cpu(), |p| {
            p.accumulated_cpu_time()
        });
        millis * 1_000_000
    }
}

impl Measurement for CpuTime {
    type Intermediate = u64;
    type Value = u64;

    fn start(&self) -> u64 {
        self.now()
    }
    fn end(&self, start: u64) -> u64 {
        // Criterion rejects a zero sample, and an idle process often uses no
        // measurable CPU; 1 ns is 0.0000001% of a one-second iteration.
        self.now().saturating_sub(start).max(1)
    }
    fn add(&self, a: &u64, b: &u64) -> u64 {
        a + b
    }
    fn zero(&self) -> u64 {
        0
    }
    fn to_f64(&self, value: &u64) -> f64 {
        *value as f64
    }
    fn formatter(&self) -> &dyn ValueFormatter {
        &Scaled {
            unit: "ms CPU",
            divisor: 1e6,
        }
    }
}

/// Growth of this process's resident memory, in bytes; negative when it
/// shrinks.
pub struct RssGrowth(ThisProcess);

impl Default for RssGrowth {
    fn default() -> Self {
        Self(ThisProcess::new())
    }
}

impl RssGrowth {
    fn now(&self) -> i64 {
        let bytes = self
            .0
            .read(ProcessRefreshKind::nothing().with_memory(), |p| p.memory());
        i64::try_from(bytes).expect("RSS fits in i64")
    }
}

impl Measurement for RssGrowth {
    type Intermediate = i64;
    type Value = i64;

    fn start(&self) -> i64 {
        self.now()
    }
    fn end(&self, start: i64) -> i64 {
        // Criterion rejects a zero sample, and an idle minute often changes
        // nothing; 1 byte stands in for "no change".
        match self.now() - start {
            0 => 1,
            growth => growth,
        }
    }
    fn add(&self, a: &i64, b: &i64) -> i64 {
        a + b
    }
    fn zero(&self) -> i64 {
        0
    }
    fn to_f64(&self, value: &i64) -> f64 {
        *value as f64
    }
    fn formatter(&self) -> &dyn ValueFormatter {
        &Scaled {
            unit: "KiB",
            divisor: 1024.0,
        }
    }
}

/// Shows values in one fixed unit.
struct Scaled {
    unit: &'static str,
    divisor: f64,
}

impl ValueFormatter for Scaled {
    fn scale_values(&self, _typical: f64, values: &mut [f64]) -> &'static str {
        for value in values {
            *value /= self.divisor;
        }
        self.unit
    }

    fn scale_throughputs(&self, typical: f64, _: &Throughput, values: &mut [f64]) -> &'static str {
        self.scale_values(typical, values)
    }

    fn scale_for_machines(&self, _values: &mut [f64]) -> &'static str {
        self.unit
    }
}
