//! The MVU store: `Msg` → pure `reduce` → immutable `AppState` snapshot plus
//! `Effect`s. Effects run on worker threads and report back with a new `Msg`.
//! Never depends on GPUI.

mod store;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tafit_core::{
    ChangeKind, FileChange, GitEnvironment, GitError, RepositoryInfo, RepositoryStatus,
};

pub use store::{AppStore, StoreEvent, StoreEvents};

/// Identifies one opened Repository for the lifetime of the store; never reused,
/// so a result for a closed Repository can't land on a newer one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RepoId(u64);

impl RepoId {
    /// The id as a number, unique within the store, e.g. for UI element ids.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Loadable<T, E = String> {
    #[default]
    Loading,
    Ready(T),
    /// Loading failed; holds what is shown to the user.
    Error(E),
}

/// Identifies one `Msg::DetectGit`, so an older detection's result can't
/// replace a newer one's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GitRequestId(u64);

/// Identifies one status read of a Repository, so a slower, older read can't
/// replace a newer one's result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusRequestId(u64);

/// One of the change list's three groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ChangeGroup {
    Staged,
    Unstaged,
    Untracked,
}

impl ChangeGroup {
    /// This group's files in `status`, sorted by path.
    pub fn files(self, status: &RepositoryStatus) -> &[FileChange] {
        match self {
            Self::Staged => &status.staged,
            Self::Unstaged => &status.unstaged,
            Self::Untracked => &status.untracked,
        }
    }
}

/// One row of the change list: a file in a group. A file staged in part has
/// a row in both `Staged` and `Unstaged`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileRef {
    pub group: ChangeGroup,
    /// As in `FileChange::path`.
    pub path: String,
}

/// How `Msg::Select` changes the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectMode {
    /// Select only this file (a plain click).
    Only,
    /// Add the file, or take it out if already selected (Ctrl/Cmd-click).
    Toggle,
}

/// One Repository Tab's state.
#[derive(Clone, Debug)]
pub struct RepoState {
    pub id: RepoId,
    pub path: PathBuf,
    pub open: Loadable<RepositoryInfo>,
    /// The change list, read once the Repository opens. Shared, so cloning
    /// the state stays cheap with tens of thousands of changes.
    pub status: Loadable<Arc<RepositoryStatus>>,
    /// A status read is running; `status` still holds the previous result.
    pub status_refreshing: bool,
    /// The latest status read; only its result is kept.
    status_request: StatusRequestId,
    /// The selected rows of the change list. A new change list drops the
    /// rows it no longer has.
    pub selection: BTreeSet<FileRef>,
    /// A stage, unstage or commit is running, or the status read after it
    /// is, so `status` may not show what it did yet. Writes run one at a
    /// time; the others are ignored until this ends.
    pub writing: bool,
    /// The write finished; `writing` ends once the status read after it lands.
    write_settling: bool,
    /// The running write is a commit. Its message is frozen until it ends,
    /// since a made commit clears it.
    pub committing: bool,
    /// Why the last write failed or was refused, in full (git's output
    /// included); cleared when the next write starts.
    pub write_error: Option<String>,
    /// The message of the next commit, as typed. Kept when a commit fails;
    /// cleared once one is made.
    pub commit_message: String,
    /// Commits made from this Repository Tab. The UI empties its message box
    /// when this grows.
    pub commits_made: u64,
}

impl RepoState {
    fn new(id: RepoId, path: PathBuf) -> Self {
        Self {
            id,
            path,
            open: Loadable::Loading,
            status: Loadable::Loading,
            status_refreshing: false,
            status_request: StatusRequestId::default(),
            selection: BTreeSet::new(),
            writing: false,
            write_settling: false,
            committing: false,
            write_error: None,
            commit_message: String::new(),
            commits_made: 0,
        }
    }

    /// Why a commit can't be made now, for the user; `None` when it can.
    /// A running write isn't a reason: the commit button waits for it.
    pub fn commit_refusal(&self) -> Option<&'static str> {
        let Loadable::Ready(status) = &self.status else {
            return Some("The change list hasn't loaded yet.");
        };
        if status.staged.is_empty() && !status.merge_in_progress {
            return Some("Nothing is staged. Stage the changes to commit first.");
        }
        if self.commit_message.trim().is_empty() {
            return Some("Write a commit message first.");
        }
        None
    }

    /// Whether the row for `path` in `group` is selected.
    pub fn is_selected(&self, group: ChangeGroup, path: &str) -> bool {
        // shortcut: allocates a key per lookup; fine for the rows on screen.
        self.selection.contains(&FileRef {
            group,
            path: path.to_owned(),
        })
    }

    /// The paths of the selected rows in `groups`, as git should see them:
    /// a rename names both its sides.
    fn selected_paths(&self, groups: &[ChangeGroup]) -> Vec<String> {
        let Loadable::Ready(status) = &self.status else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        for selected in self
            .selection
            .iter()
            .filter(|selected| groups.contains(&selected.group))
        {
            let files = selected.group.files(status);
            if let Ok(ix) = files.binary_search_by(|file| file.path.cmp(&selected.path)) {
                if let ChangeKind::Renamed { from } = &files[ix].kind {
                    paths.push(from.clone());
                }
                paths.push(selected.path.clone());
            }
        }
        paths
    }

    /// Starts a write, unless one is running or the Repository isn't open.
    fn start_write(&mut self) -> bool {
        if self.writing || !matches!(self.open, Loadable::Ready(_)) {
            return false;
        }
        self.writing = true;
        self.write_error = None;
        true
    }

    /// Records how the running write went and re-reads the change list it
    /// changed; `writing` ends when that read lands.
    fn finish_write(&mut self, result: Result<(), GitError>) -> Effect {
        self.write_settling = true;
        self.write_error = result.err().map(|err| err.to_string());
        self.request_status()
    }

    /// Starts a new status read, superseding any still running.
    fn request_status(&mut self) -> Effect {
        self.status_request = StatusRequestId(self.status_request.0 + 1);
        self.status_refreshing = true;
        Effect::LoadStatus {
            repo_id: self.id,
            request: self.status_request,
            path: self.path.clone(),
        }
    }
}

/// Identifies one Window for the lifetime of the store; never reused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(u64);

impl WindowId {
    /// The Window a default `AppState` starts with.
    pub const FIRST: Self = Self(0);
}

/// What a Window shows below its tab bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Page {
    /// The Active Repository's Repository Tab.
    #[default]
    RepositoryTab,
    /// Open a Repository: a folder picker and the pinned and recent ones.
    OpenRepository,
    Settings,
}

/// One Window and its group of Repository Tabs.
#[derive(Clone, Debug)]
pub struct WindowState {
    pub id: WindowId,
    /// The Window's Repository Tabs, in tab order.
    pub tabs: Vec<RepoId>,
    /// One of `tabs`; `None` only when there are none.
    pub active_repo: Option<RepoId>,
    pub page: Page,
    /// Grows each time the Window should come to the front: a Repository
    /// opened from another Window already has its tab here.
    pub raise_requests: u64,
}

impl WindowState {
    fn new(id: WindowId) -> Self {
        Self {
            id,
            tabs: Vec::new(),
            active_repo: None,
            page: Page::RepositoryTab,
            raise_requests: 0,
        }
    }
}

/// How many recent Repositories are remembered.
pub const RECENT_LIMIT: usize = 10;

#[derive(Clone, Debug)]
pub struct AppState {
    /// Every open Repository, whichever Window its tab is in. Tab order is
    /// `WindowState::tabs`.
    pub repos: Vec<RepoState>,
    /// Open Windows, oldest first. Starts with `WindowId::FIRST`.
    pub windows: Vec<WindowState>,
    /// The Window the user last worked in.
    pub focused_window: Option<WindowId>,
    /// Repositories opened successfully, most recent first, at most
    /// `RECENT_LIMIT`.
    pub recent: Vec<PathBuf>,
    /// Repositories the user pinned, in pin order; listed before `recent`.
    // shortcut: `recent` and `pinned` live only as long as the app runs;
    // the Session (#22) will keep them between runs.
    pub pinned: Vec<PathBuf>,
    /// The system git, found at startup (`Msg::DetectGit`). An error means git
    /// could not be run at all; `GitError::GitNotFound` means it isn't installed.
    pub git: Loadable<GitEnvironment, GitError>,
    /// The latest `Msg::DetectGit`; only its result is kept.
    git_request: GitRequestId,
    next_repo_id: u64,
    next_window_id: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            repos: Vec::new(),
            windows: vec![WindowState::new(WindowId::FIRST)],
            focused_window: Some(WindowId::FIRST),
            recent: Vec::new(),
            pinned: Vec::new(),
            git: Loadable::default(),
            git_request: GitRequestId::default(),
            next_repo_id: 0,
            next_window_id: WindowId::FIRST.0 + 1,
        }
    }
}

impl AppState {
    /// The focused Window's Active Repository.
    pub fn active(&self) -> Option<&RepoState> {
        self.active_in(self.focused_window?)
    }

    /// The id of the focused Window's Active Repository.
    pub fn active_repo(&self) -> Option<RepoId> {
        self.active().map(|repo| repo.id)
    }

    /// `window`'s Active Repository.
    pub fn active_in(&self, window: WindowId) -> Option<&RepoState> {
        self.repo(self.window(window)?.active_repo?)
    }

    pub fn window(&self, id: WindowId) -> Option<&WindowState> {
        self.windows.iter().find(|window| window.id == id)
    }

    pub fn repo(&self, id: RepoId) -> Option<&RepoState> {
        self.repos.iter().find(|repo| repo.id == id)
    }

    /// `window`'s Repository Tabs, in tab order.
    pub fn tabs(&self, window: WindowId) -> impl Iterator<Item = &RepoState> {
        self.window(window)
            .into_iter()
            .flat_map(|window| &window.tabs)
            .filter_map(|&id| self.repo(id))
    }

    /// The Repositories to offer for opening: the pinned ones first (`true`),
    /// then the recent ones that aren't pinned.
    pub fn repository_list(&self) -> impl Iterator<Item = (&Path, bool)> {
        let pinned = self.pinned.iter().map(|path| (path.as_path(), true));
        let recent = self
            .recent
            .iter()
            .filter(|path| !self.pinned.contains(path))
            .map(|path| (path.as_path(), false));
        pinned.chain(recent)
    }

    fn repo_mut(&mut self, id: RepoId) -> Option<&mut RepoState> {
        self.repos.iter_mut().find(|repo| repo.id == id)
    }

    fn window_mut(&mut self, id: WindowId) -> Option<&mut WindowState> {
        self.windows.iter_mut().find(|window| window.id == id)
    }

    /// The Window holding `repo`'s tab.
    fn window_of_mut(&mut self, repo: RepoId) -> Option<&mut WindowState> {
        self.windows
            .iter_mut()
            .find(|window| window.tabs.contains(&repo))
    }
}

#[derive(Debug)]
pub enum Msg {
    /// Open the Repository at `path` in a new Repository Tab of `window` and
    /// make it active there. A Repository already open anywhere gets no
    /// second tab: its tab is made active and its Window brought forward.
    OpenRepo {
        window: WindowId,
        path: PathBuf,
    },
    /// Select this Repository's tab, making it its Window's Active Repository
    /// and that Window the focused one.
    ActivateRepo(RepoId),
    /// Close this Repository's tab, and with it the Repository. Closing the
    /// active tab activates the one after it, or else the one before.
    CloseRepo(RepoId),
    /// Move this Repository's tab to index `to` of its Window's tabs, as
    /// counted once it is taken out; past the end means last.
    MoveTab {
        repo_id: RepoId,
        to: usize,
    },
    /// Open a new, empty Window and focus it.
    OpenWindow,
    /// Close this Window and the Repositories in its tabs.
    CloseWindow(WindowId),
    /// The user started working in this Window.
    FocusWindow(WindowId),
    /// Keep this Repository at the top of the Repository list.
    PinRepo(PathBuf),
    UnpinRepo(PathBuf),
    /// Result of `Effect::OpenRepo`.
    RepoOpened {
        repo_id: RepoId,
        result: Result<RepositoryInfo, GitError>,
    },
    /// Re-read the Repository's change list (the manual refresh).
    RefreshStatus(RepoId),
    /// Result of `Effect::LoadStatus`.
    StatusLoaded {
        repo_id: RepoId,
        request: StatusRequestId,
        result: Result<RepositoryStatus, GitError>,
    },
    /// Find the system git and the signing tools it calls.
    DetectGit,
    /// Result of `Effect::DetectGit`.
    GitDetected {
        request: GitRequestId,
        result: Result<GitEnvironment, GitError>,
    },
    /// Show `page` in `window`.
    ShowPage {
        window: WindowId,
        page: Page,
    },
    /// Change which rows of the change list are selected.
    Select {
        repo_id: RepoId,
        file: FileRef,
        mode: SelectMode,
    },
    /// Stage the selected unstaged and untracked files.
    StageSelected(RepoId),
    /// Unstage the selected staged files.
    UnstageSelected(RepoId),
    /// Result of `Effect::Stage` or `Effect::Unstage`.
    IndexUpdated {
        repo_id: RepoId,
        result: Result<(), GitError>,
    },
    /// The commit message box changed.
    EditCommitMessage {
        repo_id: RepoId,
        message: String,
    },
    /// Commit what is staged with `RepoState::commit_message`; refused, with
    /// the reason in `write_error`, when `commit_refusal` gives one.
    Commit(RepoId),
    /// Result of `Effect::Commit`.
    Committed {
        repo_id: RepoId,
        result: Result<(), GitError>,
    },
    /// Result of `Effect::LoadInfo`.
    InfoLoaded {
        repo_id: RepoId,
        result: Result<RepositoryInfo, GitError>,
    },
}

/// Work the reducer asks for; executed off the store thread.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    OpenRepo {
        repo_id: RepoId,
        path: PathBuf,
    },
    /// The Repository closed: stop its running reads, whose results the
    /// reducer would drop anyway.
    CloseRepo {
        repo_id: RepoId,
    },
    DetectGit {
        request: GitRequestId,
    },
    /// Read the Repository's change list.
    LoadStatus {
        repo_id: RepoId,
        request: StatusRequestId,
        path: PathBuf,
    },
    /// Stage these repo-relative paths.
    Stage {
        repo_id: RepoId,
        path: PathBuf,
        paths: Vec<String>,
    },
    /// Unstage these repo-relative paths.
    Unstage {
        repo_id: RepoId,
        path: PathBuf,
        paths: Vec<String>,
    },
    /// Commit what is staged with this message.
    Commit {
        repo_id: RepoId,
        path: PathBuf,
        message: String,
    },
    /// Re-read what HEAD points at (after a commit moved it).
    LoadInfo {
        repo_id: RepoId,
        path: PathBuf,
    },
}

/// The pure reducer. Also the store's synchronous mode: tests and benchmarks
/// can drive an `AppState` directly and inspect the effects it asks for.
pub fn reduce(state: &mut AppState, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::OpenRepo { window, path } => {
            if let Some(open) = state.repos.iter().find(|repo| repo.path == path) {
                let repo_id = open.id;
                if let Some(window_with_tab) = state.window_of_mut(repo_id) {
                    window_with_tab.active_repo = Some(repo_id);
                    window_with_tab.page = Page::RepositoryTab;
                    if window_with_tab.id != window {
                        window_with_tab.raise_requests += 1;
                    }
                    state.focused_window = Some(window_with_tab.id);
                }
                return Vec::new();
            }
            let repo_id = RepoId(state.next_repo_id);
            let Some(target) = state.window_mut(window) else {
                return Vec::new();
            };
            target.tabs.push(repo_id);
            target.active_repo = Some(repo_id);
            target.page = Page::RepositoryTab;
            state.next_repo_id += 1;
            state.repos.push(RepoState::new(repo_id, path.clone()));
            vec![Effect::OpenRepo { repo_id, path }]
        }
        Msg::ActivateRepo(repo_id) => {
            if let Some(window) = state.window_of_mut(repo_id) {
                window.active_repo = Some(repo_id);
                window.page = Page::RepositoryTab;
                let id = window.id;
                state.focused_window = Some(id);
            }
            Vec::new()
        }
        Msg::CloseRepo(repo_id) => {
            let Some(window) = state.window_of_mut(repo_id) else {
                return Vec::new();
            };
            let ix = window
                .tabs
                .iter()
                .position(|&id| id == repo_id)
                .expect("the Window holds the tab");
            window.tabs.remove(ix);
            if window.active_repo == Some(repo_id) {
                let next = ix.min(window.tabs.len().saturating_sub(1));
                window.active_repo = window.tabs.get(next).copied();
            }
            state.repos.retain(|repo| repo.id != repo_id);
            vec![Effect::CloseRepo { repo_id }]
        }
        Msg::MoveTab { repo_id, to } => {
            if let Some(window) = state.window_of_mut(repo_id) {
                window.tabs.retain(|&id| id != repo_id);
                let to = to.min(window.tabs.len());
                window.tabs.insert(to, repo_id);
            }
            Vec::new()
        }
        Msg::OpenWindow => {
            let id = WindowId(state.next_window_id);
            state.next_window_id += 1;
            state.windows.push(WindowState::new(id));
            state.focused_window = Some(id);
            Vec::new()
        }
        Msg::CloseWindow(id) => {
            let Some(ix) = state.windows.iter().position(|window| window.id == id) else {
                return Vec::new();
            };
            let window = state.windows.remove(ix);
            state.repos.retain(|repo| !window.tabs.contains(&repo.id));
            if state.focused_window == Some(id) {
                state.focused_window = state.windows.last().map(|window| window.id);
            }
            window
                .tabs
                .into_iter()
                .map(|repo_id| Effect::CloseRepo { repo_id })
                .collect()
        }
        Msg::FocusWindow(id) => {
            if state.window(id).is_some() {
                state.focused_window = Some(id);
            }
            Vec::new()
        }
        Msg::PinRepo(path) => {
            if !state.pinned.contains(&path) {
                state.pinned.push(path);
            }
            Vec::new()
        }
        Msg::UnpinRepo(path) => {
            state.pinned.retain(|pinned| *pinned != path);
            Vec::new()
        }
        Msg::RepoOpened { repo_id, result } => {
            // A Repository closed while loading has no state left; drop the result.
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            match result {
                Ok(info) => {
                    repo.open = Loadable::Ready(info);
                    let effect = repo.request_status();
                    let path = repo.path.clone();
                    state.recent.retain(|recent| *recent != path);
                    state.recent.insert(0, path);
                    state.recent.truncate(RECENT_LIMIT);
                    vec![effect]
                }
                Err(err) => {
                    repo.open = Loadable::Error(err.to_string());
                    Vec::new()
                }
            }
        }
        Msg::StatusLoaded {
            repo_id,
            request,
            result,
        } => {
            // Only the latest read counts; an older one finishing late is stale.
            if let Some(repo) = state.repo_mut(repo_id)
                && request == repo.status_request
            {
                repo.status = match result {
                    Ok(status) => {
                        repo.selection.retain(|selected| {
                            selected
                                .group
                                .files(&status)
                                .binary_search_by(|file| file.path.cmp(&selected.path))
                                .is_ok()
                        });
                        Loadable::Ready(Arc::new(status))
                    }
                    Err(err) => Loadable::Error(err.to_string()),
                };
                repo.status_refreshing = false;
                // Only a read started after the write finished settles it;
                // one started during the write is stale by then.
                if repo.write_settling {
                    repo.write_settling = false;
                    repo.writing = false;
                    repo.committing = false;
                }
            }
            Vec::new()
        }
        Msg::RefreshStatus(repo_id) => match state.repo_mut(repo_id) {
            // An unopened Repository reads its status once it opens.
            Some(repo) if matches!(repo.open, Loadable::Ready(_)) => vec![repo.request_status()],
            _ => Vec::new(),
        },
        Msg::DetectGit => {
            let request = GitRequestId(state.git_request.0 + 1);
            state.git_request = request;
            state.git = Loadable::Loading;
            vec![Effect::DetectGit { request }]
        }
        Msg::GitDetected { request, result } => {
            // A newer detection is running; its result will land instead.
            if request == state.git_request {
                state.git = match result {
                    Ok(git) => Loadable::Ready(git),
                    Err(err) => Loadable::Error(err),
                };
            }
            Vec::new()
        }
        Msg::ShowPage { window, page } => {
            if let Some(window) = state.window_mut(window) {
                window.page = page;
            }
            Vec::new()
        }
        Msg::Select {
            repo_id,
            file,
            mode,
        } => {
            if let Some(repo) = state.repo_mut(repo_id) {
                match mode {
                    SelectMode::Only => {
                        repo.selection.clear();
                        repo.selection.insert(file);
                    }
                    SelectMode::Toggle => {
                        if !repo.selection.remove(&file) {
                            repo.selection.insert(file);
                        }
                    }
                }
            }
            Vec::new()
        }
        Msg::StageSelected(repo_id) => {
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            let paths = repo.selected_paths(&[ChangeGroup::Unstaged, ChangeGroup::Untracked]);
            if paths.is_empty() || !repo.start_write() {
                return Vec::new();
            }
            vec![Effect::Stage {
                repo_id,
                path: repo.path.clone(),
                paths,
            }]
        }
        Msg::UnstageSelected(repo_id) => {
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            let paths = repo.selected_paths(&[ChangeGroup::Staged]);
            if paths.is_empty() || !repo.start_write() {
                return Vec::new();
            }
            vec![Effect::Unstage {
                repo_id,
                path: repo.path.clone(),
                paths,
            }]
        }
        Msg::IndexUpdated { repo_id, result } => match state.repo_mut(repo_id) {
            Some(repo) => vec![repo.finish_write(result)],
            None => Vec::new(),
        },
        Msg::EditCommitMessage { repo_id, message } => {
            if let Some(repo) = state.repo_mut(repo_id)
                && !repo.committing
            {
                repo.commit_message = message;
            }
            Vec::new()
        }
        Msg::Commit(repo_id) => {
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            if repo.writing || !matches!(repo.open, Loadable::Ready(_)) {
                return Vec::new();
            }
            if let Some(refusal) = repo.commit_refusal() {
                repo.write_error = Some(refusal.to_owned());
                return Vec::new();
            }
            repo.start_write();
            repo.committing = true;
            vec![Effect::Commit {
                repo_id,
                path: repo.path.clone(),
                message: repo.commit_message.clone(),
            }]
        }
        Msg::Committed { repo_id, result } => {
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            let made = result.is_ok();
            let mut effects = vec![repo.finish_write(result)];
            if made {
                repo.commit_message.clear();
                repo.commits_made += 1;
                effects.push(Effect::LoadInfo {
                    repo_id,
                    path: repo.path.clone(),
                });
            }
            effects
        }
        Msg::InfoLoaded { repo_id, result } => {
            // shortcut: no request id; reads only follow commits, which run
            // one at a time, so an older read finishing last is unlikely.
            // A failed re-read keeps the last HEAD; the status read after
            // the commit reports a Repository that went bad.
            if let Some(repo) = state.repo_mut(repo_id)
                && let Ok(info) = result
            {
                repo.open = Loadable::Ready(info);
            }
            Vec::new()
        }
    }
}
