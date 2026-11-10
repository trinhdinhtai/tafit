//! A Repository's history list: the index of every commit, read whole on a
//! worker thread, and the labels of the refs it starts from.

use std::path::PathBuf;
use std::sync::Arc;

use tafit_core::{CommitId, GitError, History, HistoryIndex, RefLabel, RefLabels};

use crate::{Effect, Loadable, RepoId};

/// Identifies one history read of a Repository, so an older read can't
/// replace a newer one's result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HistoryRequestId(u64);

/// One Repository's history list.
#[derive(Clone, Debug, Default)]
pub struct HistoryState {
    /// Every commit, newest first. While a newer read runs, the previous
    /// result stays.
    pub index: Loadable<Arc<HistoryIndex>>,
    /// The labels of the refs the `Ready` index was walked from.
    pub refs: Arc<RefLabels>,
    /// A newer read runs; `index` still holds the previous one.
    pub refreshing: bool,
    /// The latest read; only its result is kept.
    request: HistoryRequestId,
}

impl HistoryState {
    /// How many commits the history has; 0 until it has loaded.
    pub fn len(&self) -> usize {
        match &self.index {
            Loadable::Ready(index) => index.len(),
            _ => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The commit at `row`.
    pub fn commit_id(&self, row: usize) -> Option<CommitId> {
        match &self.index {
            Loadable::Ready(index) => index.id(row),
            _ => None,
        }
    }

    /// The refs on the commit at `row`, in the order they are shown.
    pub fn labels(&self, row: usize) -> &[RefLabel] {
        self.commit_id(row).map_or(&[], |id| self.refs.of(id))
    }

    /// Starts a new read, superseding any still running.
    pub(crate) fn request(&mut self, repo_id: RepoId, path: PathBuf) -> Effect {
        self.request = HistoryRequestId(self.request.0 + 1);
        self.refreshing = true;
        Effect::LoadHistory {
            repo_id,
            request: self.request,
            path,
        }
    }

    /// Takes the result of read `request`, unless a newer read started.
    pub(crate) fn loaded(&mut self, request: HistoryRequestId, result: Result<History, GitError>) {
        if request != self.request {
            return;
        }
        self.refreshing = false;
        match result {
            Ok(history) => {
                self.index = Loadable::Ready(Arc::new(history.index));
                self.refs = Arc::new(history.refs);
            }
            Err(err) => self.index = Loadable::Error(err.to_string()),
        }
    }
}
