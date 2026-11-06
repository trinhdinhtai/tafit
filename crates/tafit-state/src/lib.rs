//! The MVU store: `Msg` → pure `reduce` → immutable `AppState` snapshot plus
//! `Effect`s. Effects run on worker threads and report back with a new `Msg`.
//! Never depends on GPUI.

mod store;

use std::path::PathBuf;
use std::sync::Arc;

use tafit_core::{GitEnvironment, GitError, RepositoryInfo, RepositoryStatus};

pub use store::{AppStore, StoreEvent, StoreEvents};

/// Identifies one opened Repository for the lifetime of the store; never reused,
/// so a result for a closed Repository can't land on a newer one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RepoId(u64);

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
}

impl RepoState {
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

#[derive(Clone, Debug, Default)]
pub struct AppState {
    /// Open Repositories, in Repository Tab order.
    pub repos: Vec<RepoState>,
    pub active_repo: Option<RepoId>,
    /// The system git, found at startup (`Msg::DetectGit`). An error means git
    /// could not be run at all; `GitError::GitNotFound` means it isn't installed.
    pub git: Loadable<GitEnvironment, GitError>,
    /// The latest `Msg::DetectGit`; only its result is kept.
    git_request: GitRequestId,
    /// Whether the settings page is shown in place of the Repository Tab.
    pub settings_open: bool,
    next_repo_id: u64,
}

impl AppState {
    /// The Active Repository's state.
    pub fn active(&self) -> Option<&RepoState> {
        let id = self.active_repo?;
        self.repos.iter().find(|repo| repo.id == id)
    }

    fn repo_mut(&mut self, id: RepoId) -> Option<&mut RepoState> {
        self.repos.iter_mut().find(|repo| repo.id == id)
    }
}

#[derive(Debug)]
pub enum Msg {
    /// Open the Repository at this path in a new Repository Tab and make it active.
    OpenRepo(PathBuf),
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
    OpenSettings,
    CloseSettings,
}

/// Work the reducer asks for; executed off the store thread.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    OpenRepo {
        repo_id: RepoId,
        path: PathBuf,
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
}

/// The pure reducer. Also the store's synchronous mode: tests and benchmarks
/// can drive an `AppState` directly and inspect the effects it asks for.
pub fn reduce(state: &mut AppState, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::OpenRepo(path) => {
            let repo_id = RepoId(state.next_repo_id);
            state.next_repo_id += 1;
            state.repos.push(RepoState {
                id: repo_id,
                path: path.clone(),
                open: Loadable::Loading,
                status: Loadable::Loading,
                status_refreshing: false,
                status_request: StatusRequestId::default(),
            });
            state.active_repo = Some(repo_id);
            vec![Effect::OpenRepo { repo_id, path }]
        }
        Msg::RepoOpened { repo_id, result } => {
            // A Repository closed while loading has no state left; drop the result.
            let Some(repo) = state.repo_mut(repo_id) else {
                return Vec::new();
            };
            match result {
                Ok(info) => {
                    repo.open = Loadable::Ready(info);
                    vec![repo.request_status()]
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
                    Ok(status) => Loadable::Ready(Arc::new(status)),
                    Err(err) => Loadable::Error(err.to_string()),
                };
                repo.status_refreshing = false;
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
        Msg::OpenSettings => {
            state.settings_open = true;
            Vec::new()
        }
        Msg::CloseSettings => {
            state.settings_open = false;
            Vec::new()
        }
    }
}
