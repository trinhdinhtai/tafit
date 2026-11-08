//! Watches a Repository's files for changes made outside the app and tells
//! the store what they touched (`Msg::FilesChanged`), in batches.
//!
//! Events are gathered until none has come for `debounce`, or for at most
//! `max_batch`, then classified by where they happened (the work tree, the
//! index, HEAD and branches, tags, config). Work tree files git ignores are
//! dropped. A watch that can't cover everything is degraded: the Repository
//! is then re-read every `idle_tick`, and the watch set up again every
//! `retry`.

use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};
use tafit_core::{CancellationToken, GitBackend, GitRepository, RepositoryPaths};

use crate::{ChangeFlags, ExternalChanges, Msg, RepoId, Watch, WatchSession};

/// How watches batch events and recover. `Default` is what the app uses.
#[derive(Clone, Debug)]
pub struct WatchOptions {
    /// A batch ends once no event has come for this long...
    pub debounce: Duration,
    /// ...or once its first event is this old.
    pub max_batch: Duration,
    /// How often a degraded watch re-reads the whole Repository.
    pub idle_tick: Duration,
    /// How often a degraded watch is set up again.
    pub retry: Duration,
    /// Watch each folder on its own rather than the work tree as a whole,
    /// skipping ignored folders: inotify (Linux) has no recursive watch.
    pub per_folder: bool,
    /// With `per_folder`, a Repository with more folders than this is only
    /// watched in part, and the watch is degraded.
    pub max_folders: usize,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(250),
            max_batch: Duration::from_secs(2),
            idle_tick: Duration::from_secs(30),
            retry: Duration::from_secs(120),
            per_folder: cfg!(target_os = "linux"),
            max_folders: 4096,
        }
    }
}

/// A batch holds at most this many work tree paths; past it, the work tree
/// counts as changed without asking which files are ignored.
// shortcut: a build writing more files than this into an ignored folder
// within one batch refreshes the change list once.
const MAX_BATCH_FILES: usize = 20_000;

/// Folders of a git dir whose changes never show: skipped by `per_folder`.
const UNWATCHED_GIT_FOLDERS: [&str; 6] =
    ["objects", "logs", "hooks", "lfs", "modules", "worktrees"];

/// A running watch; dropping it stops the watch.
pub(crate) struct WatchHandle {
    inputs: mpsc::Sender<Input>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        let _ = self.inputs.send(Input::Stop);
    }
}

enum Input {
    Event(notify::Result<notify::Event>),
    Stop,
}

/// Prepares a watch of the Repository at `path`, reporting to the store
/// through `report`: the handle that stops it, and the job that starts it
/// on its own thread. Starting a thread takes a while, so the store hands
/// the job to a worker instead of starting it itself; a watch stopped
/// before it starts ends at once.
pub(crate) fn prepare(
    backend: Arc<dyn GitBackend>,
    repo_id: RepoId,
    session: WatchSession,
    path: PathBuf,
    options: WatchOptions,
    report: impl Fn(Msg) + Send + 'static,
) -> (WatchHandle, impl FnOnce() + Send + 'static) {
    let (inputs, receiver) = mpsc::channel();
    let sender = inputs.clone();
    let start = move || {
        let run = move || {
            let mut watch = RepoWatch {
                backend,
                path,
                options,
                inputs: sender,
                setup: None,
                batch: Batch::default(),
            };
            let set_watch = |watch| {
                report(Msg::WatchChanged {
                    repo_id,
                    session,
                    watch,
                })
            };
            let changed = |changes| {
                report(Msg::FilesChanged {
                    repo_id,
                    session,
                    changes,
                })
            };
            watch.run(&receiver, &set_watch, &changed);
        };
        thread::Builder::new()
            .name(format!("tafit-watch-{}", repo_id.as_u64()))
            .spawn(run)
            .expect("spawn watch thread");
    };
    (WatchHandle { inputs }, start)
}

struct RepoWatch {
    backend: Arc<dyn GitBackend>,
    path: PathBuf,
    options: WatchOptions,
    /// Handed to each notify watcher for its events.
    inputs: mpsc::Sender<Input>,
    setup: Option<Setup>,
    batch: Batch,
}

/// A watch set up over one opened Repository.
struct Setup {
    repo: Arc<dyn GitRepository>,
    layout: Layout,
    watcher: notify::RecommendedWatcher,
    /// The folders watched one by one (`per_folder`).
    folders: HashSet<PathBuf>,
    /// Why not everything is watched.
    degraded: Option<String>,
}

impl RepoWatch {
    fn run(
        &mut self,
        inputs: &mpsc::Receiver<Input>,
        set_watch: &dyn Fn(Watch),
        changed: &dyn Fn(ExternalChanges),
    ) {
        self.set_up(set_watch);
        let mut next_tick = Instant::now() + self.options.idle_tick;
        let mut next_retry = Instant::now() + self.options.retry;
        loop {
            let degraded = self.degraded();
            let mut deadline = self.batch.deadline(&self.options);
            if degraded {
                deadline = Some(
                    deadline
                        .map_or(next_tick, |at| at.min(next_tick))
                        .min(next_retry),
                );
            }
            let input = match deadline {
                Some(at) => inputs.recv_timeout(at.saturating_duration_since(Instant::now())),
                None => inputs
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            match input {
                Ok(Input::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(Input::Event(event)) => self.add(event),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            if self
                .batch
                .deadline(&self.options)
                .is_some_and(|at| at <= now)
                && let Some(changes) = self.flush()
            {
                changed(changes);
            }
            if !degraded {
                next_tick = now + self.options.idle_tick;
                next_retry = now + self.options.retry;
                continue;
            }
            if now >= next_tick {
                next_tick = now + self.options.idle_tick;
                changed(ExternalChanges::everything());
            }
            if now >= next_retry {
                next_retry = now + self.options.retry;
                self.setup = None;
                self.set_up(set_watch);
                // Whatever changed while the watch was down.
                if !self.degraded() {
                    changed(ExternalChanges::everything());
                }
            }
        }
    }

    fn degraded(&self) -> bool {
        self.setup
            .as_ref()
            .is_none_or(|setup| setup.degraded.is_some())
    }

    fn set_up(&mut self, set_watch: &dyn Fn(Watch)) {
        let watch = match self.try_set_up() {
            Ok(setup) => {
                let watch = match &setup.degraded {
                    Some(reason) => Watch::Degraded(reason.clone()),
                    None => Watch::Watching,
                };
                self.setup = Some(setup);
                watch
            }
            Err(reason) => Watch::Degraded(reason),
        };
        set_watch(watch);
    }

    fn try_set_up(&self) -> Result<Setup, String> {
        let cancel = CancellationToken::default();
        let repo = self
            .backend
            .open(&self.path, &cancel)
            .map_err(|err| err.to_string())?;
        let layout = Layout::new(&repo.paths());
        let inputs = self.inputs.clone();
        let watcher = notify::recommended_watcher(move |event| {
            let _ = inputs.send(Input::Event(event));
        })
        .map_err(|err| format!("Can't watch files: {err}"))?;
        let mut setup = Setup {
            repo,
            layout,
            watcher,
            folders: HashSet::new(),
            degraded: None,
        };
        let roots = setup.layout.roots();
        for root in roots {
            let result = if self.options.per_folder {
                setup.watch_tree(&root, self.options.max_folders)
            } else {
                setup
                    .watcher
                    .watch(&root, RecursiveMode::Recursive)
                    .map_err(|err| format!("Can't watch {}: {err}", root.display()))
            };
            if let Err(reason) = result {
                setup.degraded.get_or_insert(reason);
            }
        }
        Ok(setup)
    }

    fn add(&mut self, event: notify::Result<notify::Event>) {
        let now = Instant::now();
        let event = match event {
            Ok(event) => event,
            // Events may have been lost: read everything.
            Err(_) => {
                self.batch.everything(now);
                return;
            }
        };
        if event.need_rescan() {
            self.batch.everything(now);
            return;
        }
        if matches!(event.kind, EventKind::Access(_)) {
            return;
        }
        let Some(setup) = &mut self.setup else {
            return;
        };
        for path in &event.paths {
            let is_dir = path.is_dir();
            // A folder created or moved in has no watch of its own yet.
            let mut watch_folder =
                self.options.per_folder && is_dir && !setup.folders.contains(path);
            match setup.layout.classify(path) {
                Touched::Nothing => watch_folder = false,
                Touched::Git(flags) => {
                    if !flags.is_empty() {
                        self.batch.add_flags(flags, now);
                    }
                    watch_folder &= !setup.layout.unwatched_git_folder(path);
                }
                Touched::Worktree(mut rel) => {
                    if is_dir {
                        rel.push('/');
                    }
                    if watch_folder {
                        watch_folder = !setup
                            .repo
                            .ignored(std::slice::from_ref(&rel), &CancellationToken::default())
                            .is_ok_and(|ignored| ignored.first() == Some(&true));
                    }
                    self.batch.add_file(rel, now);
                }
            }
            if watch_folder && let Err(reason) = setup.watch_tree(path, self.options.max_folders) {
                setup.degraded.get_or_insert(reason);
            }
            if matches!(event.kind, EventKind::Remove(_)) {
                setup.folders.remove(path);
            }
        }
    }

    /// Ends the batch: what it touched, or `None` when only ignored files.
    fn flush(&mut self) -> Option<ExternalChanges> {
        let batch = std::mem::take(&mut self.batch);
        if batch.everything {
            return Some(ExternalChanges::everything());
        }
        let mut flags = batch.flags;
        let mut files = None;
        if batch.too_many_files {
            flags |= ChangeFlags::WORKTREE;
        } else if !batch.files.is_empty() {
            let setup = self.setup.as_mut()?;
            // New config may change what is ignored: read it afresh.
            if flags.contains(ChangeFlags::CONFIG)
                && let Ok(repo) = self.backend.open(&self.path, &CancellationToken::default())
            {
                setup.repo = repo;
            }
            let paths: Vec<String> = batch.files.into_iter().collect();
            let kept: BTreeSet<String> =
                match setup.repo.ignored(&paths, &CancellationToken::default()) {
                    Ok(ignored) => paths
                        .into_iter()
                        .zip(ignored)
                        .filter(|(_, ignored)| !ignored)
                        .map(|(path, _)| path)
                        .collect(),
                    // Can't tell: count them all.
                    Err(_) => paths.into_iter().collect(),
                };
            if !kept.is_empty() {
                flags |= ChangeFlags::WORKTREE;
                files = Some(
                    kept.into_iter()
                        .map(|path| path.trim_end_matches('/').to_owned())
                        .collect(),
                );
            }
        }
        (!flags.is_empty()).then_some(ExternalChanges { flags, files })
    }
}

impl Setup {
    /// Watches `root` and the folders under it one by one, skipping what git
    /// ignores and the git dir's folders that never show. Stops, with the
    /// reason, past `max` folders.
    fn watch_tree(&mut self, root: &Path, max: usize) -> Result<(), String> {
        let mut queue = VecDeque::from([root.to_owned()]);
        while let Some(folder) = queue.pop_front() {
            if self.folders.contains(&folder) {
                continue;
            }
            if self.folders.len() >= max {
                return Err(format!(
                    "This Repository has more than {max} folders to watch; changes in some may be found late."
                ));
            }
            self.watcher
                .watch(&folder, RecursiveMode::NonRecursive)
                .map_err(|err| format!("Can't watch {}: {err}", folder.display()))?;
            self.folders.insert(folder.clone());
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            let mut worktree = Vec::new();
            for entry in entries.flatten() {
                if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let path = entry.path();
                match self.layout.classify(&path) {
                    Touched::Worktree(mut rel) => {
                        rel.push('/');
                        worktree.push((rel, path));
                    }
                    Touched::Git(_) => {
                        if !self.layout.unwatched_git_folder(&path) {
                            queue.push_back(path);
                        }
                    }
                    Touched::Nothing => {}
                }
            }
            if worktree.is_empty() {
                continue;
            }
            let rels: Vec<String> = worktree.iter().map(|(rel, _)| rel.clone()).collect();
            let ignored = self
                .repo
                .ignored(&rels, &CancellationToken::default())
                .unwrap_or_else(|_| vec![false; rels.len()]);
            queue.extend(
                worktree
                    .into_iter()
                    .zip(ignored)
                    .filter(|(_, ignored)| !ignored)
                    .map(|((_, path), _)| path),
            );
        }
        Ok(())
    }
}

/// Events gathered since the last flush.
#[derive(Default)]
struct Batch {
    flags: ChangeFlags,
    /// Work tree paths, repo-relative; a folder ends in `/`.
    files: BTreeSet<String>,
    too_many_files: bool,
    /// Events were lost; anything may have changed.
    everything: bool,
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Batch {
    fn touch(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }

    fn add_flags(&mut self, flags: ChangeFlags, now: Instant) {
        self.flags |= flags;
        self.touch(now);
    }

    fn add_file(&mut self, rel: String, now: Instant) {
        if self.files.len() < MAX_BATCH_FILES {
            self.files.insert(rel);
        } else {
            self.too_many_files = true;
        }
        self.touch(now);
    }

    fn everything(&mut self, now: Instant) {
        self.everything = true;
        self.touch(now);
    }

    /// When the batch ends; `None` when it is empty.
    fn deadline(&self, options: &WatchOptions) -> Option<Instant> {
        let (first, last) = (self.first?, self.last?);
        Some((last + options.debounce).min(first + options.max_batch))
    }
}

/// What an event's path is to the Repository.
#[derive(Debug, PartialEq, Eq)]
enum Touched {
    /// A file or folder in the work tree, repo-relative with `/` separators,
    /// as the file system spells it.
    Worktree(String),
    /// Something in a git dir; empty flags for what never shows.
    Git(ChangeFlags),
    Nothing,
}

/// Where a Repository's files are, each place under every spelling events
/// may use: as given and with links resolved (FSEvents reports the latter).
struct Layout {
    workdir: Vec<PathBuf>,
    git_dir: Vec<PathBuf>,
    common_dir: Vec<PathBuf>,
}

impl Layout {
    fn new(paths: &RepositoryPaths) -> Self {
        Self {
            workdir: paths.workdir.as_deref().map(spellings).unwrap_or_default(),
            git_dir: spellings(&paths.git_dir),
            common_dir: spellings(&paths.common_dir),
        }
    }

    /// The folders to watch: the work tree, and each git dir outside it.
    fn roots(&self) -> Vec<PathBuf> {
        let all: Vec<&PathBuf> = [&self.workdir, &self.git_dir, &self.common_dir]
            .into_iter()
            .filter_map(|dir| dir.first())
            .collect();
        let mut roots: Vec<PathBuf> = Vec::new();
        for (ix, path) in all.iter().enumerate() {
            // Skip a folder inside another root, or the same as an earlier one.
            let covered = all.iter().enumerate().any(|(other_ix, other)| {
                other_ix != ix
                    && relative(other, path).is_some()
                    && (relative(path, other).is_none() || other_ix < ix)
            });
            if !covered {
                roots.push((*path).clone());
            }
        }
        roots
    }

    fn classify(&self, path: &Path) -> Touched {
        let path = strip_verbatim(path);
        let shared = self.git_dir.first() == self.common_dir.first();
        if let Some(rel) = relative_to_any(&self.git_dir, &path) {
            let mut flags = own_git_file(&rel);
            if shared {
                flags |= common_git_file(&rel);
            }
            return Touched::Git(flags);
        }
        if let Some(rel) = relative_to_any(&self.common_dir, &path) {
            return Touched::Git(common_git_file(&rel));
        }
        match relative_to_any(&self.workdir, &path) {
            Some(rel) if rel.is_empty() || rel == ".git" || rel.starts_with(".git/") => {
                Touched::Nothing
            }
            Some(rel) => Touched::Worktree(rel),
            None => Touched::Nothing,
        }
    }

    /// Whether `path` is a folder of a git dir whose changes never show.
    fn unwatched_git_folder(&self, path: &Path) -> bool {
        let path = strip_verbatim(path);
        [&self.git_dir, &self.common_dir].iter().any(|dir| {
            relative_to_any(dir, &path)
                .is_some_and(|rel| UNWATCHED_GIT_FOLDERS.contains(&rel.as_str()))
        })
    }
}

/// `path` as given, made absolute, and with links resolved, without the
/// Windows `\\?\` prefix.
fn spellings(path: &Path) -> Vec<PathBuf> {
    let given = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    let mut spellings = vec![strip_verbatim(&given)];
    if let Ok(resolved) = std::fs::canonicalize(path) {
        let resolved = strip_verbatim(&resolved);
        if !spellings.contains(&resolved) {
            spellings.push(resolved);
        }
    }
    spellings
}

/// What the work tree's own HEAD, index and operations in progress touch.
fn own_git_file(rel: &str) -> ChangeFlags {
    if rel.split('/').any(|part| part.ends_with(".lock")) {
        return ChangeFlags::default();
    }
    let top = rel.split('/').next().unwrap_or_default();
    match rel {
        "index" => ChangeFlags::INDEX,
        "HEAD" | "MERGE_HEAD" | "CHERRY_PICK_HEAD" | "REVERT_HEAD" | "REBASE_HEAD" => {
            ChangeFlags::GIT_STATE
        }
        "config.worktree" => ChangeFlags::CONFIG,
        _ if matches!(top, "rebase-merge" | "rebase-apply" | "sequencer") => ChangeFlags::GIT_STATE,
        _ => ChangeFlags::default(),
    }
}

/// What the files every Worktree shares touch: refs and config.
fn common_git_file(rel: &str) -> ChangeFlags {
    if rel.split('/').any(|part| part.ends_with(".lock")) {
        return ChangeFlags::default();
    }
    match rel {
        "packed-refs" => ChangeFlags::GIT_STATE | ChangeFlags::TAG,
        "config" | "info/exclude" => ChangeFlags::CONFIG,
        _ if rel.starts_with("refs/tags/") => ChangeFlags::TAG,
        _ if rel.starts_with("refs/") => ChangeFlags::GIT_STATE,
        _ => ChangeFlags::default(),
    }
}

fn relative_to_any(roots: &[PathBuf], path: &Path) -> Option<String> {
    roots.iter().find_map(|root| relative(root, path))
}

/// `path` relative to `root` with `/` separators, spelled as in `path`;
/// `None` when it isn't under `root`. Case-insensitive where the file
/// system usually is (Windows, macOS).
fn relative(root: &Path, path: &Path) -> Option<String> {
    let mut parts = path.components();
    for root_part in root.components() {
        if !same_component(root_part, parts.next()?) {
            return None;
        }
    }
    let rest: Vec<_> = parts
        .map(|part| part.as_os_str().to_string_lossy())
        .collect();
    Some(rest.join("/"))
}

fn same_component(a: Component<'_>, b: Component<'_>) -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// `path` without Windows' `\\?\` (or `\\?\UNC\`) prefix, which events and
/// `canonicalize` may add but the paths the user opened don't have.
fn strip_verbatim(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_owned();
    };
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(workdir: &str, git_dir: &str, common_dir: &str) -> Layout {
        Layout {
            workdir: vec![PathBuf::from(workdir)],
            git_dir: vec![PathBuf::from(git_dir)],
            common_dir: vec![PathBuf::from(common_dir)],
        }
    }

    fn root() -> &'static str {
        if cfg!(windows) {
            r"C:\work\repo"
        } else {
            "/work/repo"
        }
    }

    fn at(rel: &str) -> PathBuf {
        let mut path = PathBuf::from(root());
        path.extend(rel.split('/'));
        path
    }

    fn normal() -> Layout {
        let git = at(".git");
        layout(root(), git.to_str().unwrap(), git.to_str().unwrap())
    }

    #[test]
    fn files_in_the_git_dir_are_flagged_by_what_they_hold() {
        let layout = normal();
        let cases = [
            (".git/index", ChangeFlags::INDEX),
            (".git/HEAD", ChangeFlags::GIT_STATE),
            (".git/MERGE_HEAD", ChangeFlags::GIT_STATE),
            (".git/rebase-merge/done", ChangeFlags::GIT_STATE),
            (".git/refs/heads/main", ChangeFlags::GIT_STATE),
            (".git/refs/remotes/origin/main", ChangeFlags::GIT_STATE),
            (".git/refs/tags/v1", ChangeFlags::TAG),
            (
                ".git/packed-refs",
                ChangeFlags::GIT_STATE | ChangeFlags::TAG,
            ),
            (".git/config", ChangeFlags::CONFIG),
            (".git/info/exclude", ChangeFlags::CONFIG),
            (".git/index.lock", ChangeFlags::default()),
            (".git/refs/heads/main.lock", ChangeFlags::default()),
            (".git/objects/ab/cdef", ChangeFlags::default()),
            (".git/logs/HEAD", ChangeFlags::default()),
            (".git/FETCH_HEAD", ChangeFlags::default()),
        ];
        for (rel, flags) in cases {
            assert_eq!(layout.classify(&at(rel)), Touched::Git(flags), "{rel}");
        }
    }

    #[test]
    fn work_tree_files_keep_their_spelling() {
        let layout = normal();
        assert_eq!(
            layout.classify(&at("src/Main.RS")),
            Touched::Worktree("src/Main.RS".to_owned())
        );
        assert_eq!(layout.classify(&at("")), Touched::Nothing);
        let outside = if cfg!(windows) {
            r"C:\elsewhere\a"
        } else {
            "/elsewhere/a"
        };
        assert_eq!(layout.classify(Path::new(outside)), Touched::Nothing);
    }

    #[test]
    fn a_linked_worktree_has_its_own_head_and_index_but_shared_refs() {
        let main = at(".git");
        let own = at(".git/worktrees/linked");
        let workdir = if cfg!(windows) {
            r"C:\work\linked"
        } else {
            "/work/linked"
        };
        let layout = layout(workdir, own.to_str().unwrap(), main.to_str().unwrap());

        assert_eq!(
            layout.classify(&at(".git/worktrees/linked/index")),
            Touched::Git(ChangeFlags::INDEX)
        );
        assert_eq!(
            layout.classify(&at(".git/worktrees/linked/HEAD")),
            Touched::Git(ChangeFlags::GIT_STATE)
        );
        // The main work tree's index and HEAD aren't this one's.
        assert_eq!(
            layout.classify(&at(".git/index")),
            Touched::Git(ChangeFlags::default())
        );
        assert_eq!(
            layout.classify(&at(".git/HEAD")),
            Touched::Git(ChangeFlags::default())
        );
        assert_eq!(
            layout.classify(&at(".git/refs/heads/x")),
            Touched::Git(ChangeFlags::GIT_STATE)
        );
        assert_eq!(layout.roots().len(), 2);
    }

    #[test]
    fn the_work_tree_and_its_git_dir_are_watched_from_one_root() {
        assert_eq!(normal().roots(), [PathBuf::from(root())]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_paths_match_without_the_verbatim_prefix_and_in_any_case() {
        let layout = normal();
        assert_eq!(
            layout.classify(Path::new(r"\\?\C:\work\repo\Src\A.txt")),
            Touched::Worktree("Src/A.txt".to_owned())
        );
        assert_eq!(
            layout.classify(Path::new(r"c:\WORK\Repo\.git\index")),
            Touched::Git(ChangeFlags::INDEX)
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\server\share\x")),
            PathBuf::from(r"\\server\share\x")
        );
    }

    #[test]
    fn a_batch_ends_after_a_quiet_spell_or_at_its_longest() {
        let options = WatchOptions::default();
        let start = Instant::now();
        let mut batch = Batch::default();
        assert_eq!(batch.deadline(&options), None);

        batch.add_flags(ChangeFlags::INDEX, start);
        assert_eq!(batch.deadline(&options), Some(start + options.debounce));

        // Events keep coming: the batch still ends by `max_batch`.
        let mut now = start;
        while now < start + Duration::from_secs(3) {
            now += Duration::from_millis(100);
            batch.add_file("a.txt".to_owned(), now);
        }
        assert_eq!(batch.deadline(&options), Some(start + options.max_batch));
    }
}
