//! The MVU store: `Msg` → pure `reduce` → immutable `AppState` snapshot plus
//! `Effect`s. Effects run on worker threads and report back with a new `Msg`.
//! Never depends on GPUI.

mod store;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tafit_core::{
    ChangeKind, DiffSource, FileChange, FileDiff, GitEnvironment, GitError, LineAction,
    LinePatchError, LineRef, LineSelection, RepositoryInfo, RepositoryStatus, WhitespaceMode,
};

pub use store::{AppStore, StoreEvent, StoreEvents};

/// Identifies one opened Repository for the lifetime of the store; never reused,
/// so a result for a closed Repository can't land on a newer one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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

/// Identifies one diff read of a Repository, so an older read (of another
/// file, or of the same file before it changed) can't replace a newer one's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffRequestId(u64);

/// Identifies one watch of a Repository, so what an older watch reports
/// after it stopped can't land on a newer one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WatchSession(u64);

/// What a batch of changes made outside the app touched, so only the parts
/// of a Repository it affects are re-read.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct ChangeFlags(u8);

impl ChangeFlags {
    /// A file in the work tree that git doesn't ignore.
    pub const WORKTREE: Self = Self(1);
    /// The index.
    pub const INDEX: Self = Self(1 << 1);
    /// HEAD, a branch, or an operation in progress (merge, rebase, ...).
    pub const GIT_STATE: Self = Self(1 << 2);
    pub const TAG: Self = Self(1 << 3);
    /// git config or ignore rules outside the work tree (`info/exclude`).
    pub const CONFIG: Self = Self(1 << 4);
    pub const ALL: Self = Self(0b1_1111);

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for ChangeFlags {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOrAssign for ChangeFlags {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl std::fmt::Debug for ChangeFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = [
            (Self::WORKTREE, "WORKTREE"),
            (Self::INDEX, "INDEX"),
            (Self::GIT_STATE, "GIT_STATE"),
            (Self::TAG, "TAG"),
            (Self::CONFIG, "CONFIG"),
        ];
        let set: Vec<_> = names
            .iter()
            .filter(|(flag, _)| self.contains(*flag))
            .map(|(_, name)| *name)
            .collect();
        write!(f, "ChangeFlags({})", set.join(" | "))
    }
}

/// Changes made outside the app to a watched Repository, gathered over one
/// batch of file events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalChanges {
    pub flags: ChangeFlags,
    /// The changed work tree files, repo-relative with `/` separators;
    /// `None` when not known one by one (too many, or events were lost).
    pub files: Option<BTreeSet<String>>,
}

impl ExternalChanges {
    /// Anything may have changed: re-read it all.
    pub fn everything() -> Self {
        Self {
            flags: ChangeFlags::ALL,
            files: None,
        }
    }
}

/// How a Repository's files are watched for changes made outside the app.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Watch {
    /// Not watched: it isn't an Active Repository, or isn't open. Changes
    /// made meanwhile are read when it next becomes active.
    #[default]
    Off,
    /// The watch is being set up.
    Starting,
    Watching,
    /// Some or all of it can't be watched, for this reason. Until a retry
    /// succeeds, the Repository is re-read every so often instead.
    Degraded(String),
}

/// How a Line Diff is laid out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffLayout {
    /// Removed and added lines in one column, in git's hunks.
    #[default]
    Inline,
    /// The whole file, old beside new, aligned line by line.
    TwoColumn,
}

/// How Line Diffs are shown; the same for every Repository.
// shortcut: lives only as long as the app runs; the Session (#22) will keep
// it between runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffOptions {
    pub layout: DiffLayout,
    pub whitespace: WhitespaceMode,
    /// Long lines wrap instead of being cut off.
    pub word_wrap: bool,
    /// Columns a tab advances to the next multiple of.
    pub tab_size: u8,
    /// The two columns scroll together.
    pub sync_scroll: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            layout: DiffLayout::default(),
            whitespace: WhitespaceMode::default(),
            word_wrap: false,
            tab_size: 4,
            sync_scroll: true,
        }
    }
}

/// The Line Diff of the one selected file.
#[derive(Clone, Debug)]
pub struct DiffState {
    pub file: FileRef,
    /// While a newer read of the same file runs, the previous result stays.
    pub diff: Loadable<Arc<FileDiff>>,
    /// The changed lines selected in the `Ready` diff. A result that differs
    /// from it clears them.
    pub lines: LineSelection,
    /// What git is given for the file: it, or a rename's old path then new.
    paths: Vec<String>,
    /// The layout and whitespace mode the `Ready` diff was read with.
    read_with: (DiffLayout, WhitespaceMode),
    /// A newer read of the file runs; `diff` still holds the previous one.
    pub refreshing: bool,
}

/// Changed lines of one file to stage, unstage or Discard, with the diff
/// they were selected in. Applying them is refused as stale once that diff
/// is no longer the file's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineChange {
    pub file: FileRef,
    pub lines: LineSelection,
    paths: Vec<String>,
    seen: Arc<FileDiff>,
    layout: DiffLayout,
    whitespace: WhitespaceMode,
}

impl ChangeGroup {
    /// Whether `action` applies to a diff of this group: staging to unstaged
    /// and untracked files, unstaging to staged ones, Discard to unstaged.
    pub fn allows(self, action: LineAction) -> bool {
        matches!(
            (self, action),
            (Self::Unstaged | Self::Untracked, LineAction::Stage)
                | (Self::Staged, LineAction::Unstage)
                | (Self::Unstaged, LineAction::Discard)
        )
    }

    fn diff_source(self) -> DiffSource {
        match self {
            Self::Staged => DiffSource::Staged,
            Self::Unstaged => DiffSource::Unstaged,
            Self::Untracked => DiffSource::Untracked,
        }
    }
}

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
    /// The diff of the selected file; `None` unless exactly one file (not
    /// an untracked folder) is selected.
    pub diff: Option<DiffState>,
    /// The latest diff read; only its result is kept.
    diff_request: DiffRequestId,
    /// The layout and whitespace mode of the latest diff read.
    diff_requested_with: (DiffLayout, WhitespaceMode),
    /// Lines waiting for the user to confirm their Discard, which git can't
    /// undo (`Msg::ConfirmDiscard`, `Msg::CancelDiscard`).
    pub confirm_discard: Option<LineChange>,
    /// Whether changes made outside the app are seen as they happen.
    pub watch: Watch,
    /// The latest watch; only what it reports counts.
    watch_session: WatchSession,
    /// The shown diff may have changed: re-read it once the latest status
    /// read lands.
    reread_diff: bool,
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
            diff: None,
            diff_request: DiffRequestId::default(),
            diff_requested_with: Default::default(),
            confirm_discard: None,
            watch: Watch::Off,
            watch_session: WatchSession::default(),
            reread_diff: false,
        }
    }

    /// Re-reads what `changes` may have changed. Nothing is read for a
    /// Repository that isn't open.
    fn read_changes(&mut self, changes: &ExternalChanges) -> Vec<Effect> {
        if !matches!(self.open, Loadable::Ready(_)) {
            return Vec::new();
        }
        let mut effects = Vec::new();
        let status_flags = ChangeFlags::WORKTREE
            | ChangeFlags::INDEX
            | ChangeFlags::GIT_STATE
            | ChangeFlags::CONFIG;
        if changes.flags.intersects(status_flags) {
            let reread_diff = self.diff_changed(changes);
            effects.push(self.request_status(reread_diff));
        }
        if changes.flags.contains(ChangeFlags::GIT_STATE) {
            effects.push(Effect::LoadInfo {
                repo_id: self.id,
                path: self.path.clone(),
            });
        }
        // shortcut: nothing shows tags yet; the history (#14) will re-read
        // them on `ChangeFlags::TAG`.
        effects
    }

    /// Whether `changes` may have changed the shown diff.
    fn diff_changed(&self, changes: &ExternalChanges) -> bool {
        let Some(diff) = &self.diff else {
            return false;
        };
        if changes
            .flags
            .intersects(ChangeFlags::INDEX | ChangeFlags::GIT_STATE | ChangeFlags::CONFIG)
        {
            return true;
        }
        // HEAD against the index doesn't change with the work tree.
        if diff.file.group == ChangeGroup::Staged {
            return false;
        }
        match &changes.files {
            Some(files) => diff.paths.iter().any(|path| files.contains(path)),
            None => true,
        }
    }

    /// Points the diff at the one selected file, reading it if it is new or
    /// `reread` (its content, or how it is laid out, may have changed).
    /// `None` when nothing is read.
    fn sync_diff(&mut self, reread: bool, options: DiffOptions) -> Option<Effect> {
        let Loadable::Ready(status) = &self.status else {
            self.diff = None;
            return None;
        };
        let mut selected = self.selection.iter();
        let file = match (selected.next(), selected.next()) {
            (Some(file), None) if !file.path.ends_with('/') => file.clone(),
            _ => {
                self.diff = None;
                return None;
            }
        };
        let same_file = self.diff.as_ref().is_some_and(|diff| diff.file == file);
        if same_file && !reread {
            return None;
        }
        let files = file.group.files(status);
        let Ok(ix) = files.binary_search_by(|change| change.path.cmp(&file.path)) else {
            self.diff = None;
            return None;
        };
        let mut paths = Vec::with_capacity(2);
        if let ChangeKind::Renamed { from } = &files[ix].kind {
            paths.push(from.clone());
        }
        paths.push(file.path.clone());
        if !same_file {
            self.diff = Some(DiffState {
                file: file.clone(),
                diff: Loadable::Loading,
                lines: LineSelection::new(),
                paths: paths.clone(),
                read_with: (options.layout, options.whitespace),
                refreshing: false,
            });
        }
        if let Some(diff) = &mut self.diff {
            diff.refreshing = true;
        }
        self.diff_request = DiffRequestId(self.diff_request.0 + 1);
        self.diff_requested_with = (options.layout, options.whitespace);
        Some(Effect::LoadDiff {
            repo_id: self.id,
            request: self.diff_request,
            path: self.path.clone(),
            source: file.group.diff_source(),
            paths,
            layout: options.layout,
            whitespace: options.whitespace,
        })
    }

    /// `lines` of the shown diff, once it has loaded.
    fn line_change(&self, lines: LineSelection) -> Option<LineChange> {
        let diff = self.diff.as_ref()?;
        let Loadable::Ready(seen) = &diff.diff else {
            return None;
        };
        let (layout, whitespace) = diff.read_with;
        Some(LineChange {
            file: diff.file.clone(),
            lines,
            paths: diff.paths.clone(),
            seen: Arc::clone(seen),
            layout,
            whitespace,
        })
    }

    /// Asks to apply `action` to `lines` of the shown diff: a Discard waits
    /// for confirming, the others start at once. Nothing happens when the
    /// diff hasn't loaded or `action` doesn't apply to its group; an empty
    /// selection is refused in `write_error`.
    fn apply_lines(&mut self, lines: LineSelection, action: LineAction) -> Option<Effect> {
        let change = self
            .line_change(lines)
            .filter(|change| change.file.group.allows(action))?;
        if self.writing {
            return None;
        }
        if change.lines.is_empty() {
            self.write_error = Some(LinePatchError::Empty.to_string());
            return None;
        }
        if action == LineAction::Discard {
            self.confirm_discard = Some(change);
            return None;
        }
        self.start_lines(change, action)
    }

    fn start_lines(&mut self, change: LineChange, action: LineAction) -> Option<Effect> {
        self.start_write().then(|| Effect::ApplyLines {
            repo_id: self.id,
            path: self.path.clone(),
            change,
            action,
        })
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
        self.request_status(true)
    }

    /// Starts a new status read, superseding any still running; the shown
    /// diff is re-read after it if `reread_diff` (or a superseded read
    /// asked for that).
    fn request_status(&mut self, reread_diff: bool) -> Effect {
        self.reread_diff |= reread_diff;
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
    pub diff_options: DiffOptions,
    /// The system git, found at startup (`Msg::DetectGit`). An error means git
    /// could not be run at all; `GitError::GitNotFound` means it isn't installed.
    pub git: Loadable<GitEnvironment, GitError>,
    /// The latest `Msg::DetectGit`; only its result is kept.
    git_request: GitRequestId,
    /// The Repositories being watched, by id.
    watched: BTreeSet<RepoId>,
    next_watch_session: u64,
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
            diff_options: DiffOptions::default(),
            git: Loadable::default(),
            git_request: GitRequestId::default(),
            watched: BTreeSet::new(),
            next_watch_session: 0,
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

    /// Watches each Window's Active Repository once it is open and stops
    /// watching the rest. A Repository watched again is re-read: it may have
    /// changed while it wasn't watched.
    fn sync_watches(&mut self) -> Vec<Effect> {
        let wanted: BTreeSet<RepoId> = self
            .windows
            .iter()
            .filter_map(|window| window.active_repo)
            .filter(|&id| {
                self.repo(id)
                    .is_some_and(|repo| matches!(repo.open, Loadable::Ready(_)))
            })
            .collect();
        if wanted == self.watched {
            return Vec::new();
        }
        let mut effects = Vec::new();
        for &repo_id in self.watched.difference(&wanted) {
            effects.push(Effect::Unwatch { repo_id });
        }
        let started: Vec<RepoId> = wanted.difference(&self.watched).copied().collect();
        for repo in &mut self.repos {
            if !wanted.contains(&repo.id) {
                repo.watch = Watch::Off;
            }
        }
        for repo_id in started {
            let session = WatchSession(self.next_watch_session);
            self.next_watch_session += 1;
            let repo = self.repo_mut(repo_id).expect("a wanted Repository is open");
            repo.watch = Watch::Starting;
            repo.watch_session = session;
            effects.push(Effect::Watch {
                repo_id,
                session,
                path: repo.path.clone(),
            });
            // A first status read is still running when it has just opened.
            if !matches!(repo.status, Loadable::Loading) {
                effects.extend(repo.read_changes(&ExternalChanges::everything()));
            }
        }
        self.watched = wanted;
        effects
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
    /// Change which changed lines of the shown diff are selected; a line
    /// that isn't one of its changes is ignored.
    SelectLine {
        repo_id: RepoId,
        line: LineRef,
        mode: SelectMode,
    },
    /// Apply `action` to the Line Selection of the shown diff. A Discard
    /// waits in `RepoState::confirm_discard` until confirmed.
    ApplyLines {
        repo_id: RepoId,
        action: LineAction,
    },
    /// Apply `action` to every changed line of hunk `hunk` (an index into
    /// the shown inline diff's hunks), as `ApplyLines` does.
    ApplyHunk {
        repo_id: RepoId,
        hunk: usize,
        action: LineAction,
    },
    /// Go ahead with the Discard in `RepoState::confirm_discard`.
    ConfirmDiscard(RepoId),
    /// Drop the Discard in `RepoState::confirm_discard`.
    CancelDiscard(RepoId),
    /// Result of `Effect::Stage`, `Effect::Unstage` or `Effect::ApplyLines`.
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
    /// Change how Line Diffs are shown. A new layout or whitespace mode
    /// re-reads the diffs shown, which keep their previous result until it
    /// lands.
    SetDiffOptions(DiffOptions),
    /// Result of `Effect::LoadDiff`.
    DiffLoaded {
        repo_id: RepoId,
        request: DiffRequestId,
        result: Result<FileDiff, GitError>,
    },
    /// Result of `Effect::LoadInfo`.
    InfoLoaded {
        repo_id: RepoId,
        result: Result<RepositoryInfo, GitError>,
    },
    /// A watch started by `Effect::Watch` is set up, degraded or recovered.
    WatchChanged {
        repo_id: RepoId,
        session: WatchSession,
        watch: Watch,
    },
    /// A watch started by `Effect::Watch` saw changes made outside the app;
    /// what they touched is re-read.
    FilesChanged {
        repo_id: RepoId,
        session: WatchSession,
        changes: ExternalChanges,
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
    /// Apply `action` to the changed lines, unless the file's diff is no
    /// longer the one they were selected in.
    ApplyLines {
        repo_id: RepoId,
        path: PathBuf,
        change: LineChange,
        action: LineAction,
    },
    /// Commit what is staged with this message.
    Commit {
        repo_id: RepoId,
        path: PathBuf,
        message: String,
    },
    /// Read the Line Diff of one file.
    LoadDiff {
        repo_id: RepoId,
        request: DiffRequestId,
        path: PathBuf,
        source: DiffSource,
        /// The file, or a rename's old path then new path.
        paths: Vec<String>,
        layout: DiffLayout,
        whitespace: WhitespaceMode,
    },
    /// Re-read what HEAD points at (after a commit moved it).
    LoadInfo {
        repo_id: RepoId,
        path: PathBuf,
    },
    /// Watch the Repository's files for changes made outside the app,
    /// reporting with `Msg::WatchChanged` and `Msg::FilesChanged`.
    Watch {
        repo_id: RepoId,
        session: WatchSession,
        path: PathBuf,
    },
    /// Stop watching the Repository.
    Unwatch {
        repo_id: RepoId,
    },
}

/// The pure reducer. Also the store's synchronous mode: tests and benchmarks
/// can drive an `AppState` directly and inspect the effects it asks for.
pub fn reduce(state: &mut AppState, msg: Msg) -> Vec<Effect> {
    let mut effects = reduce_msg(state, msg);
    effects.extend(state.sync_watches());
    effects
}

fn reduce_msg(state: &mut AppState, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::WatchChanged {
            repo_id,
            session,
            watch,
        } => {
            if let Some(repo) = state.repo_mut(repo_id)
                && repo.watch != Watch::Off
                && repo.watch_session == session
            {
                repo.watch = watch;
            }
            Vec::new()
        }
        Msg::FilesChanged {
            repo_id,
            session,
            changes,
        } => match state.repo_mut(repo_id) {
            Some(repo) if repo.watch != Watch::Off && repo.watch_session == session => {
                repo.read_changes(&changes)
            }
            _ => Vec::new(),
        },
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
                    let effect = repo.request_status(true);
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
            let options = state.diff_options;
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
                // The file shown may have changed along with the list.
                let reread = std::mem::take(&mut repo.reread_diff);
                return repo.sync_diff(reread, options).into_iter().collect();
            }
            Vec::new()
        }
        Msg::DiffLoaded {
            repo_id,
            request,
            result,
        } => {
            if let Some(repo) = state.repo_mut(repo_id)
                && request == repo.diff_request
                && let Some(diff) = &mut repo.diff
            {
                let unchanged = matches!(
                    (&diff.diff, &result),
                    (Loadable::Ready(shown), Ok(loaded)) if **shown == *loaded
                );
                if !unchanged {
                    diff.lines.clear();
                    diff.diff = match result {
                        Ok(loaded) => Loadable::Ready(Arc::new(loaded)),
                        Err(err) => Loadable::Error(err.to_string()),
                    };
                }
                diff.read_with = repo.diff_requested_with;
                diff.refreshing = false;
            }
            Vec::new()
        }
        Msg::SelectLine {
            repo_id,
            line,
            mode,
        } => {
            if let Some(repo) = state.repo_mut(repo_id)
                && let Some(diff) = &mut repo.diff
                && let Loadable::Ready(shown) = &diff.diff
                && shown.has_change(line)
            {
                match mode {
                    SelectMode::Only => {
                        diff.lines.clear();
                        diff.lines.insert(line);
                    }
                    SelectMode::Toggle => {
                        if !diff.lines.remove(&line) {
                            diff.lines.insert(line);
                        }
                    }
                }
            }
            Vec::new()
        }
        Msg::ApplyLines { repo_id, action } => {
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            let lines = repo
                .diff
                .as_ref()
                .map(|diff| diff.lines.clone())
                .unwrap_or_default();
            repo.apply_lines(lines, action).into_iter().collect()
        }
        Msg::ApplyHunk {
            repo_id,
            hunk,
            action,
        } => {
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            let lines = match repo.diff.as_ref().map(|diff| &diff.diff) {
                Some(Loadable::Ready(shown)) => match &**shown {
                    FileDiff::Text(hunks) => hunks.get(hunk).map(|hunk| hunk.changed_lines()),
                    _ => None,
                },
                _ => None,
            };
            match lines {
                Some(lines) => repo.apply_lines(lines, action).into_iter().collect(),
                None => Vec::new(),
            }
        }
        Msg::ConfirmDiscard(repo_id) => {
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            // Kept while another write runs, so the user can confirm again.
            if repo.writing {
                return Vec::new();
            }
            match repo.confirm_discard.take() {
                Some(change) => repo
                    .start_lines(change, LineAction::Discard)
                    .into_iter()
                    .collect(),
                None => Vec::new(),
            }
        }
        Msg::CancelDiscard(repo_id) => {
            if let Some(repo) = state.repo_mut(repo_id) {
                repo.confirm_discard = None;
            }
            Vec::new()
        }
        Msg::SetDiffOptions(options) => {
            let old = state.diff_options;
            state.diff_options = options;
            if (options.layout, options.whitespace) == (old.layout, old.whitespace) {
                return Vec::new();
            }
            state
                .repos
                .iter_mut()
                .filter_map(|repo| repo.sync_diff(true, options))
                .collect()
        }
        Msg::RefreshStatus(repo_id) => match state.repo_mut(repo_id) {
            // An unopened Repository reads its status once it opens.
            Some(repo) if matches!(repo.open, Loadable::Ready(_)) => {
                vec![repo.request_status(true)]
            }
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
            let options = state.diff_options;
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
                return repo.sync_diff(false, options).into_iter().collect();
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
