//! Status reads finishing out of order, driven through the synchronous
//! reducer: real reads can't be made to finish in a chosen order.

use std::path::PathBuf;
use std::sync::Arc;

use tafit_core::{ChangeKind, FileChange, Head, RepositoryInfo, RepositoryStatus};
use tafit_state::{AppState, Effect, Loadable, Msg, RepoId, StatusRequestId, reduce};

/// A state with one opened Repository, and the status read that opening started.
fn opened() -> (AppState, RepoId, StatusRequestId) {
    let mut state = AppState::default();
    reduce(&mut state, Msg::OpenRepo(PathBuf::from("/work/repo")));
    let repo_id = state.active_repo.expect("OpenRepo activates the new tab");
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
    (state, repo_id, only_status_read(effects))
}

fn only_status_read(effects: Vec<Effect>) -> StatusRequestId {
    match &effects[..] {
        [Effect::LoadStatus { request, .. }] => *request,
        other => panic!("expected one status read, got {other:?}"),
    }
}

/// A change list with one untracked file at `path`.
fn untracked(path: &str) -> RepositoryStatus {
    RepositoryStatus {
        untracked: vec![FileChange {
            path: path.to_owned(),
            kind: ChangeKind::Added,
        }],
        ..RepositoryStatus::default()
    }
}

fn loaded(repo_id: RepoId, request: StatusRequestId, status: RepositoryStatus) -> Msg {
    Msg::StatusLoaded {
        repo_id,
        request,
        result: Ok(status),
    }
}

fn shown(state: &AppState) -> &Loadable<Arc<RepositoryStatus>> {
    &state.active().expect("a Repository is open").status
}

#[test]
fn a_refresh_keeps_showing_the_last_list_until_it_finishes() {
    let (mut state, repo_id, first) = opened();
    reduce(&mut state, loaded(repo_id, first, untracked("a.txt")));

    let refresh = only_status_read(reduce(&mut state, Msg::RefreshStatus(repo_id)));

    let repo = state.active().expect("a Repository is open");
    assert!(repo.status_refreshing);
    assert_eq!(repo.status, Loadable::Ready(Arc::new(untracked("a.txt"))));

    reduce(&mut state, loaded(repo_id, refresh, untracked("b.txt")));

    let repo = state.active().expect("a Repository is open");
    assert!(!repo.status_refreshing);
    assert_eq!(repo.status, Loadable::Ready(Arc::new(untracked("b.txt"))));
}

#[test]
fn an_older_read_finishing_after_a_newer_one_is_dropped() {
    let (mut state, repo_id, first) = opened();
    let second = only_status_read(reduce(&mut state, Msg::RefreshStatus(repo_id)));

    reduce(&mut state, loaded(repo_id, second, untracked("new.txt")));
    reduce(&mut state, loaded(repo_id, first, untracked("old.txt")));

    assert_eq!(
        shown(&state),
        &Loadable::Ready(Arc::new(untracked("new.txt")))
    );
}

#[test]
fn an_older_read_finishing_first_does_not_end_the_refresh() {
    let (mut state, repo_id, first) = opened();
    reduce(&mut state, Msg::RefreshStatus(repo_id));

    reduce(&mut state, loaded(repo_id, first, untracked("old.txt")));

    let repo = state.active().expect("a Repository is open");
    assert_eq!(repo.status, Loadable::Loading);
    assert!(repo.status_refreshing);
}

#[test]
fn a_repository_that_has_not_opened_is_not_refreshed() {
    let mut state = AppState::default();
    reduce(&mut state, Msg::OpenRepo(PathBuf::from("/work/repo")));
    let repo_id = state.active_repo.expect("OpenRepo activates the new tab");

    assert_eq!(reduce(&mut state, Msg::RefreshStatus(repo_id)), []);
}
