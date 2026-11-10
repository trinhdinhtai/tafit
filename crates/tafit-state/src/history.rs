//! A Repository's history list: the index of every commit, read whole on a
//! worker thread, the labels of the refs it starts from, and what is shown
//! about each commit, read a window of rows at a time as they come on
//! screen.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

use tafit_core::{CommitId, CommitSummary, GitError, History, HistoryIndex, RefLabel, RefLabels};

use crate::{Effect, Loadable, RepoId};

/// How many rows of the history one metadata read covers. Window `w` is
/// rows `w * HISTORY_WINDOW..(w + 1) * HISTORY_WINDOW`.
pub const HISTORY_WINDOW: usize = 256;

/// Windows kept loaded on each side of the shown ones, so scrolling back a
/// little shows them at once. The rest are dropped.
const WINDOWS_KEPT_AROUND: usize = 4;

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
    /// The read the `Ready` index came from; a window read for another
    /// index is dropped.
    shown_request: HistoryRequestId,
    /// The windows read or being read for the `Ready` index, by number.
    windows: BTreeMap<usize, Loadable<Arc<[CommitSummary]>>>,
    /// The rows on screen, as the UI last told.
    shown: Range<usize>,
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

    /// What is shown about the commit at `row`, once its window has loaded.
    pub fn commit(&self, row: usize) -> Option<&CommitSummary> {
        match self.windows.get(&history_window(row))? {
            Loadable::Ready(commits) => commits.get(row % HISTORY_WINDOW),
            _ => None,
        }
    }

    /// Why the window holding `row` couldn't be read, if it couldn't.
    pub fn commit_error(&self, row: usize) -> Option<&str> {
        match self.windows.get(&history_window(row))? {
            Loadable::Error(err) => Some(err),
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

    /// Takes the result of read `request`, unless a newer read started, and
    /// reads the shown rows of the new index. A window whose commits are
    /// the same in the new index stays.
    pub(crate) fn loaded(
        &mut self,
        repo_id: RepoId,
        path: PathBuf,
        request: HistoryRequestId,
        result: Result<History, GitError>,
    ) -> Vec<Effect> {
        if request != self.request {
            return Vec::new();
        }
        self.refreshing = false;
        match result {
            Ok(history) => {
                let index = history.index;
                self.windows.retain(|&window, commits| match commits {
                    Loadable::Ready(commits) => commits
                        .iter()
                        .map(|commit| commit.id)
                        .eq(index.ids(history_window_rows(window)).iter().copied()),
                    _ => false,
                });
                self.index = Loadable::Ready(Arc::new(index));
                self.refs = Arc::new(history.refs);
                self.shown_request = request;
                self.read_shown(repo_id, path)
            }
            Err(err) => {
                self.index = Loadable::Error(err.to_string());
                self.windows.clear();
                Vec::new()
            }
        }
    }

    /// The UI shows `rows`: reads the windows they are in that aren't read
    /// yet, and drops windows far from them.
    pub(crate) fn show_rows(
        &mut self,
        repo_id: RepoId,
        path: PathBuf,
        rows: Range<usize>,
    ) -> Vec<Effect> {
        self.shown = rows;
        self.read_shown(repo_id, path)
    }

    fn read_shown(&mut self, repo_id: RepoId, path: PathBuf) -> Vec<Effect> {
        let Loadable::Ready(index) = &self.index else {
            return Vec::new();
        };
        let end = self.shown.end.min(index.len());
        if self.shown.start >= end {
            return Vec::new();
        }
        let (first, last) = (history_window(self.shown.start), history_window(end - 1));
        self.windows.retain(|&window, _| {
            window + WINDOWS_KEPT_AROUND >= first && window <= last + WINDOWS_KEPT_AROUND
        });
        let mut effects = Vec::new();
        for window in first..=last {
            if self.windows.contains_key(&window) {
                continue;
            }
            self.windows.insert(window, Loadable::Loading);
            effects.push(Effect::LoadCommits {
                repo_id,
                request: self.shown_request,
                window,
                path: path.clone(),
                ids: index.ids(history_window_rows(window)).to_vec(),
            });
        }
        effects
    }

    /// Takes the result of reading `window` of the index read `request`
    /// gave, unless that index is no longer shown or the window was dropped.
    pub(crate) fn commits_loaded(
        &mut self,
        request: HistoryRequestId,
        window: usize,
        result: Result<Vec<CommitSummary>, GitError>,
    ) {
        if request != self.shown_request {
            return;
        }
        if let Some(slot @ Loadable::Loading) = self.windows.get_mut(&window) {
            *slot = match result {
                Ok(commits) => Loadable::Ready(commits.into()),
                Err(err) => Loadable::Error(err.to_string()),
            };
        }
    }
}

/// The window holding `row`.
pub fn history_window(row: usize) -> usize {
    row / HISTORY_WINDOW
}

/// The rows of window `window`, past the end of the history for its last
/// window.
pub fn history_window_rows(window: usize) -> Range<usize> {
    window * HISTORY_WINDOW..(window + 1) * HISTORY_WINDOW
}
