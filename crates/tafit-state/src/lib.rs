//! The MVU store: `Msg` → pure `reduce` → immutable `AppState` snapshot plus
//! `Effect`s. Effects run on worker threads and report back with a new `Msg`.
//! Never depends on GPUI.

mod store;

use std::path::PathBuf;

use tafit_core::{GitError, RepositoryInfo};

pub use store::{AppStore, StoreEvent, StoreEvents};

/// Identifies one opened Repository for the lifetime of the store; never reused,
/// so a result for a closed Repository can't land on a newer one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RepoId(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loadable<T> {
    Loading,
    Ready(T),
    /// Loading failed; holds the message shown to the user.
    Error(String),
}

/// One Repository Tab's state.
#[derive(Clone, Debug)]
pub struct RepoState {
    pub id: RepoId,
    pub path: PathBuf,
    pub open: Loadable<RepositoryInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct AppState {
    /// Open Repositories, in Repository Tab order.
    pub repos: Vec<RepoState>,
    pub active_repo: Option<RepoId>,
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
}

/// Work the reducer asks for; executed off the store thread.
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    OpenRepo { repo_id: RepoId, path: PathBuf },
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
            });
            state.active_repo = Some(repo_id);
            vec![Effect::OpenRepo { repo_id, path }]
        }
        Msg::RepoOpened { repo_id, result } => {
            // A Repository closed while loading has no state left; drop the result.
            if let Some(repo) = state.repo_mut(repo_id) {
                repo.open = match result {
                    Ok(info) => Loadable::Ready(info),
                    Err(err) => Loadable::Error(err.to_string()),
                };
            }
            Vec::new()
        }
    }
}
