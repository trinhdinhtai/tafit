use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock, mpsc};
use std::thread;

use tafit_core::{
    CancellationToken, DiffContext, DiffSource, FileDiff, GitBackend, GitError, GitRepository,
    LineAction, LinePatchError, WhitespaceMode, line_patch,
};

use crate::watch::{self, WatchHandle, WatchOptions};
use crate::{AppState, Effect, LineChange, Msg, RepoId, reduce};

/// Sent to the UI whenever a new snapshot is published. Coalesced: several
/// publications may arrive as one event, so read `AppStore::snapshot` after it.
#[derive(Debug)]
pub enum StoreEvent {
    StateChanged,
}

/// The receiving end of the store's change events, held by the UI poller.
pub type StoreEvents = async_channel::Receiver<StoreEvent>;

enum Command {
    Msg(Msg),
    /// Answered once every command before it has been handled.
    Flush(mpsc::Sender<()>),
    Shutdown,
}

/// Owns the store thread. Dropping it stops the thread.
pub struct AppStore {
    commands: mpsc::Sender<Command>,
    state: Arc<RwLock<Arc<AppState>>>,
}

impl AppStore {
    pub fn new(backend: Arc<dyn GitBackend>) -> (Self, StoreEvents) {
        Self::with_state(backend, AppState::default())
    }

    /// Starts from `initial` instead of an empty state (restored Session, tests).
    pub fn with_state(backend: Arc<dyn GitBackend>, initial: AppState) -> (Self, StoreEvents) {
        Self::start(backend, initial, WatchOptions::default())
    }

    /// Watches Repositories with `options` instead of the app's (tests).
    pub fn with_watch_options(
        backend: Arc<dyn GitBackend>,
        options: WatchOptions,
    ) -> (Self, StoreEvents) {
        Self::start(backend, AppState::default(), options)
    }

    fn start(
        backend: Arc<dyn GitBackend>,
        initial: AppState,
        watch_options: WatchOptions,
    ) -> (Self, StoreEvents) {
        let (commands, receiver) = mpsc::channel();
        // Capacity 1: a pending event already tells the UI to re-read the snapshot.
        let (events_tx, events_rx) = async_channel::bounded(1);
        let state = Arc::new(RwLock::new(Arc::new(initial)));
        let worker = StoreWorker {
            backend,
            state: Arc::clone(&state),
            events: events_tx,
            commands: commands.clone(),
            repo_load: WorkerPool::new("tafit-repo-load", repo_load_threads()),
            cancel: CancellationToken::default(),
            status_reads: HashMap::new(),
            diff_reads: HashMap::new(),
            watch_options,
            watches: HashMap::new(),
        };
        thread::Builder::new()
            .name("tafit-store".to_owned())
            .spawn(move || worker.run(receiver))
            .expect("spawn store thread");
        (Self { commands, state }, events_rx)
    }

    pub fn dispatch(&self, msg: Msg) {
        // Fails only after shutdown, when nothing is left to update.
        let _ = self.commands.send(Command::Msg(msg));
    }

    /// Blocks until every `Msg` dispatched before it has been reduced and its
    /// snapshot published; the effects it asked for may still be running.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.commands.send(Command::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }

    /// The latest published state. Cheap: a read lock and an `Arc` clone.
    pub fn snapshot(&self) -> Arc<AppState> {
        read(&self.state)
    }
}

impl Drop for AppStore {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

struct StoreWorker {
    backend: Arc<dyn GitBackend>,
    state: Arc<RwLock<Arc<AppState>>>,
    events: async_channel::Sender<StoreEvent>,
    /// Effects send their completion `Msg` back through this.
    commands: mpsc::Sender<Command>,
    repo_load: WorkerPool,
    /// Cancelled at shutdown so in-flight reads stop early.
    cancel: CancellationToken,
    /// Each Repository's latest status read. A newer read cancels the older
    /// one, whose result the reducer would drop anyway.
    status_reads: HashMap<RepoId, CancellationToken>,
    /// Each Repository's latest diff read, cancelled the same way.
    diff_reads: HashMap<RepoId, CancellationToken>,
    watch_options: WatchOptions,
    /// The watched Repositories; dropping a handle stops its watch.
    watches: HashMap<RepoId, WatchHandle>,
}

impl StoreWorker {
    fn run(mut self, commands: mpsc::Receiver<Command>) {
        let mut current = read(&self.state);
        loop {
            let msg = match commands.recv() {
                Ok(Command::Msg(msg)) => msg,
                Ok(Command::Flush(done)) => {
                    let _ = done.send(());
                    continue;
                }
                Ok(Command::Shutdown) | Err(_) => break,
            };
            let effects = reduce(Arc::make_mut(&mut current), msg);
            *self.state.write().unwrap_or_else(PoisonError::into_inner) = Arc::clone(&current);
            // Full means an event is already pending; Closed means no UI listens.
            let _ = self.events.try_send(StoreEvent::StateChanged);
            for effect in effects {
                self.schedule(effect);
            }
        }
        self.cancel.cancel();
        for read in self.status_reads.values().chain(self.diff_reads.values()) {
            read.cancel();
        }
        self.watches.clear();
    }

    fn schedule(&mut self, effect: Effect) {
        let backend = Arc::clone(&self.backend);
        let commands = self.commands.clone();
        let cancel = self.cancel.clone();
        match effect {
            Effect::OpenRepo { repo_id, path } => self.repo_load.execute(move || {
                let result = backend
                    .open(&path, &cancel)
                    .and_then(|repo| repo.info(&cancel));
                let _ = commands.send(Command::Msg(Msg::RepoOpened { repo_id, result }));
            }),
            Effect::CloseRepo { repo_id } => {
                for reads in [&mut self.status_reads, &mut self.diff_reads] {
                    if let Some(read) = reads.remove(&repo_id) {
                        read.cancel();
                    }
                }
                self.watches.remove(&repo_id);
            }
            Effect::Watch {
                repo_id,
                session,
                path,
            } => {
                let (watch, start) = watch::prepare(
                    backend,
                    repo_id,
                    session,
                    path,
                    self.watch_options.clone(),
                    move |msg| {
                        let _ = commands.send(Command::Msg(msg));
                    },
                );
                self.watches.insert(repo_id, watch);
                self.repo_load.execute(start);
            }
            Effect::Unwatch { repo_id } => {
                self.watches.remove(&repo_id);
            }
            Effect::DetectGit { request } => self.repo_load.execute(move || {
                let result = backend.detect_git(&cancel);
                let _ = commands.send(Command::Msg(Msg::GitDetected { request, result }));
            }),
            Effect::LoadStatus {
                repo_id,
                request,
                path,
            } => {
                let read = CancellationToken::default();
                if let Some(older) = self.status_reads.insert(repo_id, read.clone()) {
                    older.cancel();
                }
                self.repo_load.execute(move || {
                    let result = backend
                        .open(&path, &read)
                        .and_then(|repo| repo.status(&read));
                    let _ = commands.send(Command::Msg(Msg::StatusLoaded {
                        repo_id,
                        request,
                        result,
                    }));
                });
            }
            Effect::LoadDiff {
                repo_id,
                request,
                path,
                source,
                paths,
                whitespace,
            } => {
                let read = CancellationToken::default();
                if let Some(older) = self.diff_reads.insert(repo_id, read.clone()) {
                    older.cancel();
                }
                self.repo_load.execute(move || {
                    let result = backend
                        .open(&path, &read)
                        .and_then(|repo| read_diff(&*repo, source, &paths, whitespace, &read));
                    let _ = commands.send(Command::Msg(Msg::DiffLoaded {
                        repo_id,
                        request,
                        result,
                    }));
                });
            }
            Effect::ApplyLines {
                repo_id,
                path,
                change,
                action,
            } => self.repo_load.execute(move || {
                let result = backend
                    .open(&path, &cancel)
                    .and_then(|repo| apply_lines(&*repo, &change, action, &cancel));
                let _ = commands.send(Command::Msg(Msg::IndexUpdated { repo_id, result }));
            }),
            Effect::Stage {
                repo_id,
                path,
                paths,
            } => self.repo_load.execute(move || {
                let result = backend
                    .open(&path, &cancel)
                    .and_then(|repo| repo.stage(&paths, &cancel));
                let _ = commands.send(Command::Msg(Msg::IndexUpdated { repo_id, result }));
            }),
            Effect::Unstage {
                repo_id,
                path,
                paths,
            } => self.repo_load.execute(move || {
                let result = backend
                    .open(&path, &cancel)
                    .and_then(|repo| repo.unstage(&paths, &cancel));
                let _ = commands.send(Command::Msg(Msg::IndexUpdated { repo_id, result }));
            }),
            Effect::Commit {
                repo_id,
                path,
                message,
            } => self.repo_load.execute(move || {
                let result = backend
                    .open(&path, &cancel)
                    .and_then(|repo| repo.commit(&message, &cancel));
                let _ = commands.send(Command::Msg(Msg::Committed { repo_id, result }));
            }),
            Effect::LoadInfo { repo_id, path } => self.repo_load.execute(move || {
                let result = backend
                    .open(&path, &cancel)
                    .and_then(|repo| repo.info(&cancel));
                let _ = commands.send(Command::Msg(Msg::InfoLoaded { repo_id, result }));
            }),
        }
    }
}

/// The Line Diff of one file: git's hunks with their changed words, which
/// the UI lays out in one column or two (ADR-0006).
fn read_diff(
    repo: &dyn GitRepository,
    source: DiffSource,
    paths: &[String],
    whitespace: WhitespaceMode,
    cancel: &CancellationToken,
) -> Result<FileDiff, GitError> {
    repo.diff(source, paths, DiffContext::Hunks, whitespace, cancel)
        .map(|diff| diff.with_highlights(whitespace))
}

/// Builds the patch for `change` from git's diff of the file and applies
/// it. The diff the lines were selected in is read again after that one,
/// so a file changed since the selection, even between the two reads, is
/// refused as stale rather than patched.
fn apply_lines(
    repo: &dyn GitRepository,
    change: &LineChange,
    action: LineAction,
    cancel: &CancellationToken,
) -> Result<(), GitError> {
    let source = change.file.group.diff_source();
    let output = repo.diff_patch(source, &change.paths, cancel)?;
    let now = read_diff(repo, source, &change.paths, change.whitespace, cancel)?;
    // shortcut: this compares the diff as shown, not the file's bytes: an
    // edit the diff doesn't show (whitespace while it is ignored, a line
    // ending, bytes shown lossily) passes, and so does one made after this
    // read; git apply's own context check is all that stops those.
    if now != *change.seen {
        return Err(LinePatchError::Stale.into());
    }
    let patch = line_patch(&output, &change.lines, action)?;
    repo.apply_patch(&patch, action, cancel)
}

/// The published snapshot. A panic elsewhere can't leave it half-written (it
/// is only ever replaced whole), so a poisoned lock is still safe to read.
fn read(state: &RwLock<Arc<AppState>>) -> Arc<AppState> {
    Arc::clone(&state.read().unwrap_or_else(PoisonError::into_inner))
}

fn repo_load_threads() -> usize {
    thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4))
}

type Job = Box<dyn FnOnce() + Send>;

/// A fixed set of named threads draining one job queue. Threads exit once the
/// pool is dropped and the queue is empty.
struct WorkerPool {
    jobs: mpsc::Sender<Job>,
}

impl WorkerPool {
    fn new(name: &str, threads: usize) -> Self {
        let (jobs, receiver) = mpsc::channel::<Job>();
        let receiver = Arc::new(Mutex::new(receiver));
        for ix in 0..threads {
            let receiver = Arc::clone(&receiver);
            thread::Builder::new()
                .name(format!("{name}-{ix}"))
                .spawn(move || {
                    loop {
                        // Hold the lock only while taking a job, not while running it.
                        let job = receiver
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .recv();
                        match job {
                            Ok(job) => job(),
                            Err(_) => break,
                        }
                    }
                })
                .expect("spawn worker thread");
        }
        Self { jobs }
    }

    fn execute(&self, job: impl FnOnce() + Send + 'static) {
        let _ = self.jobs.send(Box::new(job));
    }
}
