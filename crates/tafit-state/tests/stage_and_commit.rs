//! Staging, unstaging and committing through the store, checked with git.

mod support;

use std::sync::Arc;

use support::{TestRepo, TestStore};
use tafit_state::{AppState, ChangeGroup, FileRef, Loadable, Msg, RepoId, RepoState, SelectMode};

/// Opens `repo` in a fresh store once its change list has loaded.
fn open(repo: &TestRepo) -> (TestStore, RepoId) {
    let store = TestStore::new();
    store.dispatch(Msg::OpenRepo(repo.path().to_owned()));
    let state = store.wait_until(|state| state.active().is_some_and(settled));
    let repo_id = state.active_repo.expect("the opened Repository is active");
    (store, repo_id)
}

/// No write or status read is running, and the change list has loaded.
fn settled(repo: &RepoState) -> bool {
    !repo.writing && !repo.status_refreshing && matches!(repo.status, Loadable::Ready(_))
}

/// Waits until the last write and the status read after it have finished.
fn wait_settled(store: &TestStore) -> Arc<AppState> {
    store.flush();
    store.wait_until(|state| state.active().is_some_and(settled))
}

fn active(state: &AppState) -> &RepoState {
    state.active().expect("a Repository is open")
}

fn file(group: ChangeGroup, path: &str) -> FileRef {
    FileRef {
        group,
        path: path.to_owned(),
    }
}

fn select(store: &TestStore, repo_id: RepoId, file: FileRef, mode: SelectMode) {
    store.dispatch(Msg::Select {
        repo_id,
        file,
        mode,
    });
}

/// The paths git has staged, one per line, sorted.
fn staged_in_git(repo: &TestRepo) -> String {
    repo.git(&["diff", "--cached", "--name-only"])
}

#[test]
fn staging_the_selected_files_stages_them_and_nothing_else() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\n"), ("b.txt", "b\n")], "base");
    repo.write("a.txt", "changed\n");
    repo.write("b.txt", "changed\n");
    repo.write("new.txt", "new\n");
    let (store, repo_id) = open(&repo);

    select(
        &store,
        repo_id,
        file(ChangeGroup::Unstaged, "a.txt"),
        SelectMode::Only,
    );
    select(
        &store,
        repo_id,
        file(ChangeGroup::Untracked, "new.txt"),
        SelectMode::Toggle,
    );
    store.dispatch(Msg::StageSelected(repo_id));

    let state = wait_settled(&store);
    assert_eq!(staged_in_git(&repo), "a.txt\nnew.txt");
    let repo_state = active(&state);
    let Loadable::Ready(status) = &repo_state.status else {
        unreachable!("settled")
    };
    let staged: Vec<_> = status.staged.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(staged, ["a.txt", "new.txt"]);
    assert!(
        repo_state.selection.is_empty(),
        "staged files left the groups they were selected in"
    );
    assert_eq!(repo_state.write_error, None);
}

#[test]
fn staging_a_file_removed_from_disk_stages_its_deletion() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("gone.txt", "a\n")], "base");
    repo.delete("gone.txt");
    let (store, repo_id) = open(&repo);

    select(
        &store,
        repo_id,
        file(ChangeGroup::Unstaged, "gone.txt"),
        SelectMode::Only,
    );
    store.dispatch(Msg::StageSelected(repo_id));

    wait_settled(&store);
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-status"]),
        "D\tgone.txt"
    );
}

#[test]
fn unstaging_the_selected_files_leaves_the_others_staged() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\n"), ("b.txt", "b\n")], "base");
    repo.write("a.txt", "changed\n");
    repo.write("b.txt", "changed\n");
    repo.git(&["add", "a.txt", "b.txt"]);
    let (store, repo_id) = open(&repo);

    select(
        &store,
        repo_id,
        file(ChangeGroup::Staged, "a.txt"),
        SelectMode::Only,
    );
    store.dispatch(Msg::UnstageSelected(repo_id));

    wait_settled(&store);
    assert_eq!(staged_in_git(&repo), "b.txt");
    assert_eq!(
        repo.git(&["diff", "--name-only"]),
        "a.txt",
        "the edit is still in the work tree"
    );
}

#[test]
fn unstaging_before_the_first_commit_makes_the_file_untracked_again() {
    let repo = TestRepo::init("fresh");
    repo.write("a.txt", "a\n");
    repo.git(&["add", "a.txt"]);
    let (store, repo_id) = open(&repo);

    select(
        &store,
        repo_id,
        file(ChangeGroup::Staged, "a.txt"),
        SelectMode::Only,
    );
    store.dispatch(Msg::UnstageSelected(repo_id));

    let state = wait_settled(&store);
    assert_eq!(staged_in_git(&repo), "");
    let Loadable::Ready(status) = &active(&state).status else {
        unreachable!("settled")
    };
    assert_eq!(status.untracked.len(), 1);
}

#[test]
fn unstaging_a_rename_unstages_both_of_its_sides() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("old.txt", "some content that stays the same\n")], "base");
    repo.git(&["mv", "old.txt", "new.txt"]);
    let (store, repo_id) = open(&repo);

    select(
        &store,
        repo_id,
        file(ChangeGroup::Staged, "new.txt"),
        SelectMode::Only,
    );
    store.dispatch(Msg::UnstageSelected(repo_id));

    wait_settled(&store);
    assert_eq!(
        staged_in_git(&repo),
        "",
        "neither the add nor the delete is staged"
    );
}

#[test]
fn toggling_a_selected_file_unselects_it() {
    let repo = TestRepo::init("repo");
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let (store, repo_id) = open(&repo);

    select(
        &store,
        repo_id,
        file(ChangeGroup::Untracked, "a.txt"),
        SelectMode::Only,
    );
    select(
        &store,
        repo_id,
        file(ChangeGroup::Untracked, "b.txt"),
        SelectMode::Toggle,
    );
    select(
        &store,
        repo_id,
        file(ChangeGroup::Untracked, "a.txt"),
        SelectMode::Toggle,
    );

    store.flush();
    let state = store.store.snapshot();
    let selected: Vec<_> = active(&state).selection.iter().cloned().collect();
    assert_eq!(selected, [file(ChangeGroup::Untracked, "b.txt")]);
}
