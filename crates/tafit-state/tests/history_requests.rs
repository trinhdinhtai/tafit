//! History and metadata reads finishing in a chosen order, driven through
//! the synchronous reducer: real ones can't be made to finish in a chosen
//! order.

use std::path::PathBuf;

use tafit_core::{
    CommitId, CommitSummary, CommitTime, Head, History, HistoryIndex, RefLabels, RepositoryInfo,
};
use tafit_state::{
    AppState, Effect, HISTORY_WINDOW, HistoryRequestId, Msg, RepoId, WindowId, reduce,
};

/// A state with one opened Repository, and the history read that opening
/// started.
fn opened() -> (AppState, RepoId, HistoryRequestId) {
    let mut state = AppState::default();
    reduce(
        &mut state,
        Msg::OpenRepo {
            window: WindowId::FIRST,
            path: PathBuf::from("/work/repo"),
        },
    );
    let repo_id = state.active_repo().expect("OpenRepo activates the new tab");
    let effects = reduce(
        &mut state,
        Msg::RepoOpened {
            repo_id,
            result: Ok(RepositoryInfo {
                name: "repo".to_owned(),
                head: Head::Branch("main".to_owned()),
            }),
        },
    );
    let request = effects
        .iter()
        .find_map(|effect| match effect {
            Effect::LoadHistory { request, .. } => Some(*request),
            _ => None,
        })
        .expect("opening reads the history");
    (state, repo_id, request)
}

fn id(n: usize) -> CommitId {
    let mut bytes = [0; 20];
    bytes[..8].copy_from_slice(&(n as u64).to_be_bytes());
    CommitId(bytes)
}

/// A linear history of `len` commits, newest first, whose ids start at
/// `first`.
fn history(first: usize, len: usize) -> History {
    let commits = (0..len)
        .map(|row| {
            let parents = if row + 1 < len {
                vec![id(first + row + 1)]
            } else {
                Vec::new()
            };
            (id(first + row), parents)
        })
        .collect();
    History {
        index: HistoryIndex::new(commits),
        refs: RefLabels::default(),
    }
}

fn summary(id: CommitId) -> CommitSummary {
    CommitSummary {
        id,
        summary: format!("{id}"),
        author_name: "tafit test".to_owned(),
        author_email: "test@tafit.invalid".to_owned(),
        author_time: CommitTime {
            seconds: 0,
            offset: 0,
        },
    }
}

/// The window reads among `effects`, as (request, window, ids).
fn window_reads(effects: &[Effect]) -> Vec<(HistoryRequestId, usize, Vec<CommitId>)> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::LoadCommits {
                request,
                window,
                ids,
                ..
            } => Some((*request, *window, ids.clone())),
            _ => None,
        })
        .collect()
}

fn land_history(
    state: &mut AppState,
    repo_id: RepoId,
    request: HistoryRequestId,
    history: History,
) -> Vec<Effect> {
    reduce(
        state,
        Msg::HistoryLoaded {
            repo_id,
            request,
            result: Ok(history),
        },
    )
}

fn land_window(
    state: &mut AppState,
    repo_id: RepoId,
    (request, window, ids): (HistoryRequestId, usize, Vec<CommitId>),
) {
    reduce(
        state,
        Msg::CommitsLoaded {
            repo_id,
            request,
            window,
            result: Ok(ids.into_iter().map(summary).collect()),
        },
    );
}

fn show(state: &mut AppState, repo_id: RepoId, rows: std::ops::Range<usize>) -> Vec<Effect> {
    reduce(state, Msg::ShowHistoryRows { repo_id, rows })
}

fn refresh(state: &mut AppState, repo_id: RepoId) -> HistoryRequestId {
    let effects = reduce(state, Msg::RefreshHistory(repo_id));
    match &effects[..] {
        [Effect::LoadHistory { request, .. }] => *request,
        other => panic!("expected one history read, got {other:?}"),
    }
}

#[test]
fn a_window_read_for_an_older_index_is_dropped() {
    let (mut state, repo_id, first) = opened();
    land_history(&mut state, repo_id, first, history(0, 10));
    let stale = window_reads(&show(&mut state, repo_id, 0..10)).remove(0);

    // New commits on top move every row down.
    let second = refresh(&mut state, repo_id);
    let reads = window_reads(&land_history(&mut state, repo_id, second, history(100, 10)));
    land_window(&mut state, repo_id, stale);

    let repo = state.repo(repo_id).expect("open");
    assert!(repo.history.commit(0).is_none());
    // The new index's shown rows are read again.
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].2[0], id(100));
    land_window(&mut state, repo_id, reads[0].clone());
    let repo = state.repo(repo_id).expect("open");
    assert_eq!(
        repo.history.commit(0).map(|commit| commit.id),
        Some(id(100))
    );
}

#[test]
fn a_window_whose_commits_did_not_move_stays_shown_across_a_refresh() {
    let (mut state, repo_id, first) = opened();
    land_history(&mut state, repo_id, first, history(0, 10));
    let read = window_reads(&show(&mut state, repo_id, 0..10)).remove(0);
    land_window(&mut state, repo_id, read);

    // Only a ref moved: the same commits in the same rows.
    let second = refresh(&mut state, repo_id);
    let reads = window_reads(&land_history(&mut state, repo_id, second, history(0, 10)));

    assert!(reads.is_empty());
    let repo = state.repo(repo_id).expect("open");
    assert_eq!(repo.history.commit(9).map(|commit| commit.id), Some(id(9)));
}

#[test]
fn an_older_history_read_finishing_last_is_dropped() {
    let (mut state, repo_id, first) = opened();
    let second = refresh(&mut state, repo_id);

    land_history(&mut state, repo_id, second, history(100, 3));
    land_history(&mut state, repo_id, first, history(0, 5));

    let repo = state.repo(repo_id).expect("open");
    assert_eq!(repo.history.len(), 3);
    assert_eq!(repo.history.commit_id(0), Some(id(100)));
    assert!(!repo.history.refreshing);
}

#[test]
fn a_window_already_read_or_being_read_is_not_read_again() {
    let (mut state, repo_id, first) = opened();
    land_history(&mut state, repo_id, first, history(0, 3 * HISTORY_WINDOW));
    let read = window_reads(&show(&mut state, repo_id, 0..20)).remove(0);

    assert!(window_reads(&show(&mut state, repo_id, 5..30)).is_empty());
    land_window(&mut state, repo_id, read);
    assert!(window_reads(&show(&mut state, repo_id, 10..40)).is_empty());
}

#[test]
fn windows_far_from_the_shown_rows_are_dropped_and_read_again_on_return() {
    let len = 40 * HISTORY_WINDOW;
    let (mut state, repo_id, first) = opened();
    land_history(&mut state, repo_id, first, history(0, len));
    let read = window_reads(&show(&mut state, repo_id, 0..20)).remove(0);
    land_window(&mut state, repo_id, read);

    let far = window_reads(&show(&mut state, repo_id, len - 20..len));
    assert_eq!(far.len(), 1);
    assert_eq!(far[0].1, 39);
    let repo = state.repo(repo_id).expect("open");
    assert!(repo.history.commit(0).is_none());

    let back = window_reads(&show(&mut state, repo_id, 0..20));
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].1, 0);
}

#[test]
fn rows_shown_before_the_index_loads_are_read_once_it_does() {
    let (mut state, repo_id, first) = opened();
    assert!(
        window_reads(&show(
            &mut state,
            repo_id,
            HISTORY_WINDOW..HISTORY_WINDOW + 5
        ))
        .is_empty()
    );

    let reads = window_reads(&land_history(
        &mut state,
        repo_id,
        first,
        history(0, 2 * HISTORY_WINDOW),
    ));

    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].1, 1);
}
