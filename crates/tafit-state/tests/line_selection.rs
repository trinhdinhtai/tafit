//! Staging, unstaging and Discarding hunks and Line Selections through the
//! store, checked with git.

mod support;

use std::sync::Arc;

use support::{TestRepo, TestStore};
use tafit_core::{FileDiff, LineAction, LinePatchError, LineRef, LineSide};
use tafit_state::{
    AppState, ChangeGroup, DiffLayout, DiffOptions, FileRef, Loadable, Msg, RepoId, RepoState,
    SelectMode, WindowId,
};

/// Opens `repo` in a fresh store once its change list has loaded.
fn open(repo: &TestRepo) -> (TestStore, RepoId) {
    let store = TestStore::new();
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: repo.path().to_owned(),
    });
    let state = store.wait_until(|state| state.active().is_some_and(settled));
    (store, state.active_repo().expect("the opened Repository"))
}

/// No write or status read is running, the change list has loaded, and so
/// has the diff of the selected file, if any.
fn settled(repo: &RepoState) -> bool {
    !repo.writing
        && !repo.status_refreshing
        && matches!(repo.status, Loadable::Ready(_))
        && repo
            .diff
            .as_ref()
            .is_none_or(|diff| !diff.refreshing && matches!(diff.diff, Loadable::Ready(_)))
}

fn wait_settled(store: &TestStore) -> Arc<AppState> {
    store.flush();
    store.wait_until(|state| state.active().is_some_and(settled))
}

fn active(state: &AppState) -> &RepoState {
    state.active().expect("a Repository is open")
}

/// Selects only `path` in `group` and waits for its diff.
fn show(store: &TestStore, repo_id: RepoId, group: ChangeGroup, path: &str) -> Arc<FileDiff> {
    store.dispatch(Msg::Select {
        repo_id,
        file: FileRef {
            group,
            path: path.to_owned(),
        },
        mode: SelectMode::Toggle,
    });
    let state = wait_settled(store);
    let diff = active(&state).diff.as_ref().expect("one file is selected");
    match &diff.diff {
        Loadable::Ready(loaded) => Arc::clone(loaded),
        other => panic!("the diff didn't load: {other:?}"),
    }
}

fn old(number: u32) -> LineRef {
    LineRef {
        side: LineSide::Old,
        number,
    }
}

fn new(number: u32) -> LineRef {
    LineRef {
        side: LineSide::New,
        number,
    }
}

/// Adds each line to the Line Selection.
fn select_lines(store: &TestStore, repo_id: RepoId, lines: &[LineRef]) {
    for &line in lines {
        store.dispatch(Msg::SelectLine {
            repo_id,
            line,
            mode: SelectMode::Toggle,
        });
    }
}

fn apply(store: &TestStore, repo_id: RepoId, action: LineAction) -> Arc<AppState> {
    store.dispatch(Msg::ApplyLines { repo_id, action });
    wait_settled(store)
}

#[test]
fn staging_one_line_of_a_hunk_stages_only_that_line() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\ntwo\nthree\n")], "base");
    repo.write("a.txt", "one\nTWO\nthree\nfour\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    select_lines(&store, repo_id, &[new(4)]);
    let state = apply(&store, repo_id, LineAction::Stage);

    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.index_file("a.txt"), b"one\ntwo\nthree\nfour\n");
    assert_eq!(repo.work_file("a.txt"), b"one\nTWO\nthree\nfour\n");
}

/// Lines `1..=count` as `line N`, each ending in `\n`.
fn numbered(count: u32) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}

#[test]
fn lines_selected_in_several_hunks_are_staged_together() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", &numbered(20))], "base");
    let edited = numbered(20)
        .replace("line 2\n", "line 2\nadded early\n")
        .replace("line 18\n", "LINE 18\n");
    repo.write("a.txt", &edited);
    let (store, repo_id) = open(&repo);
    let diff = show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");
    let FileDiff::Text(hunks) = &*diff else {
        panic!("an inline diff")
    };
    assert_eq!(hunks.len(), 2, "the changes are far apart");

    // The early addition, and the late edit's removal without its addition.
    select_lines(&store, repo_id, &[new(3), old(18)]);
    let state = apply(&store, repo_id, LineAction::Stage);

    assert_eq!(active(&state).write_error, None);
    let expected = numbered(20)
        .replace("line 2\n", "line 2\nadded early\n")
        .replace("line 18\n", "");
    assert_eq!(
        String::from_utf8(repo.index_file("a.txt")).unwrap(),
        expected
    );
}

#[test]
fn selecting_a_range_replaces_the_line_selection_with_its_changes() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\ntwo\nthree\n")], "base");
    repo.write("a.txt", "one\nTWO\nthree\nfour\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");
    select_lines(&store, repo_id, &[new(4)]);

    // A range over the edit, its context line included.
    store.dispatch(Msg::SelectLines {
        repo_id,
        lines: [old(1), old(2), new(2), new(3)].into_iter().collect(),
    });
    let state = wait_settled(&store);

    let diff = active(&state).diff.as_ref().expect("one file is selected");
    assert_eq!(
        diff.lines,
        [old(2), new(2)].into_iter().collect(),
        "the context lines are left out and the earlier selection is gone"
    );
}

#[test]
fn unstaging_one_line_leaves_the_rest_staged_and_the_work_tree_alone() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\ntwo\nthree\n")], "base");
    repo.write("a.txt", "one\nTWO\nthree\nfour\n");
    repo.git(&["add", "a.txt"]);
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Staged, "a.txt");

    select_lines(&store, repo_id, &[old(2), new(2)]);
    let state = apply(&store, repo_id, LineAction::Unstage);

    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.index_file("a.txt"), b"one\ntwo\nthree\nfour\n");
    assert_eq!(repo.work_file("a.txt"), b"one\nTWO\nthree\nfour\n");
}

#[test]
fn discarding_a_line_waits_for_confirmation_then_undoes_it_in_the_work_tree() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\ntwo\nthree\n")], "base");
    repo.write("a.txt", "one\nTWO\nthree\nfour\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");
    select_lines(&store, repo_id, &[new(4)]);

    let state = apply(&store, repo_id, LineAction::Discard);
    let pending = active(&state)
        .confirm_discard
        .as_ref()
        .expect("the Discard waits to be confirmed");
    assert_eq!(pending.lines, [new(4)].into());
    assert_eq!(
        repo.work_file("a.txt"),
        b"one\nTWO\nthree\nfour\n",
        "nothing is discarded before confirming"
    );

    store.dispatch(Msg::ConfirmDiscard(repo_id));
    let state = wait_settled(&store);

    assert_eq!(active(&state).write_error, None);
    assert!(active(&state).confirm_discard.is_none());
    assert_eq!(repo.work_file("a.txt"), b"one\nTWO\nthree\n");
    assert_eq!(repo.index_file("a.txt"), b"one\ntwo\nthree\n");
}

#[test]
fn a_cancelled_discard_changes_nothing() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\n")], "base");
    repo.write("a.txt", "one\ntwo\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");
    store.dispatch(Msg::ApplyHunk {
        repo_id,
        hunk: 0,
        action: LineAction::Discard,
    });

    store.dispatch(Msg::CancelDiscard(repo_id));
    store.dispatch(Msg::ConfirmDiscard(repo_id));
    let state = wait_settled(&store);

    assert!(active(&state).confirm_discard.is_none());
    assert_eq!(repo.work_file("a.txt"), b"one\ntwo\n");
}

/// A 20-line file edited near its start and near its end: two hunks.
fn two_hunks(repo: &TestRepo) {
    repo.commit_files(&[("a.txt", &numbered(20))], "base");
    repo.write(
        "a.txt",
        &numbered(20)
            .replace("line 2\n", "LINE 2\n")
            .replace("line 19\n", "LINE 19\n"),
    );
}

#[test]
fn staging_a_hunk_stages_only_that_hunk() {
    let repo = TestRepo::init("repo");
    two_hunks(&repo);
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    store.dispatch(Msg::ApplyHunk {
        repo_id,
        hunk: 1,
        action: LineAction::Stage,
    });
    let state = wait_settled(&store);

    assert_eq!(active(&state).write_error, None);
    let staged = numbered(20).replace("line 19\n", "LINE 19\n");
    assert_eq!(String::from_utf8(repo.index_file("a.txt")).unwrap(), staged);
}

#[test]
fn unstaging_a_hunk_unstages_only_that_hunk() {
    let repo = TestRepo::init("repo");
    two_hunks(&repo);
    repo.git(&["add", "a.txt"]);
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Staged, "a.txt");

    store.dispatch(Msg::ApplyHunk {
        repo_id,
        hunk: 0,
        action: LineAction::Unstage,
    });
    let state = wait_settled(&store);

    assert_eq!(active(&state).write_error, None);
    let staged = numbered(20).replace("line 19\n", "LINE 19\n");
    assert_eq!(String::from_utf8(repo.index_file("a.txt")).unwrap(), staged);
}

#[test]
fn discarding_a_hunk_undoes_only_that_hunk() {
    let repo = TestRepo::init("repo");
    two_hunks(&repo);
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    store.dispatch(Msg::ApplyHunk {
        repo_id,
        hunk: 0,
        action: LineAction::Discard,
    });
    store.dispatch(Msg::ConfirmDiscard(repo_id));
    let state = wait_settled(&store);

    assert_eq!(active(&state).write_error, None);
    let kept = numbered(20).replace("line 19\n", "LINE 19\n");
    assert_eq!(String::from_utf8(repo.work_file("a.txt")).unwrap(), kept);
}

#[test]
fn staging_and_discarding_lines_of_a_crlf_file_keeps_its_line_endings() {
    let repo = TestRepo::init("repo");
    repo.write_bytes("a.txt", b"one\r\ntwo\r\n");
    repo.git(&["add", "a.txt"]);
    repo.commit("base");
    repo.write_bytes("a.txt", b"one\r\nTWO\r\nthree\r\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    select_lines(&store, repo_id, &[new(3)]);
    let state = apply(&store, repo_id, LineAction::Stage);
    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.index_file("a.txt"), b"one\r\ntwo\r\nthree\r\n");

    select_lines(&store, repo_id, &[old(2), new(2)]);
    apply(&store, repo_id, LineAction::Discard);
    store.dispatch(Msg::ConfirmDiscard(repo_id));
    let state = wait_settled(&store);
    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.work_file("a.txt"), b"one\r\ntwo\r\nthree\r\n");
}

#[test]
fn staging_a_line_after_a_last_line_without_a_newline_gives_that_line_one() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\nb")], "base");
    repo.write("a.txt", "a\nB\nc\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    // Only the added `c`: `b` stays, so it needs its newline now.
    select_lines(&store, repo_id, &[new(3)]);
    let state = apply(&store, repo_id, LineAction::Stage);

    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.index_file("a.txt"), b"a\nb\nc\n");
}

#[test]
fn staging_and_discarding_a_last_line_without_a_newline() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\n")], "base");
    repo.write("a.txt", "a\nb\nc");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    select_lines(&store, repo_id, &[new(3)]);
    let state = apply(&store, repo_id, LineAction::Stage);
    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.index_file("a.txt"), b"a\nc");

    // The index now ends in `c` without a newline; discard the added `b`.
    select_lines(&store, repo_id, &[new(2)]);
    apply(&store, repo_id, LineAction::Discard);
    store.dispatch(Msg::ConfirmDiscard(repo_id));
    let state = wait_settled(&store);
    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.work_file("a.txt"), b"a\nc");
}

#[test]
fn lines_of_a_file_changed_since_they_were_selected_are_refused_as_stale() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\ntwo\n")], "base");
    repo.write("a.txt", "one\ntwo\nthree\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");
    select_lines(&store, repo_id, &[new(3)]);

    // Edited elsewhere, with no refresh since.
    repo.write("a.txt", "zero\none\ntwo\nthree\n");
    let state = apply(&store, repo_id, LineAction::Stage);

    assert_eq!(
        active(&state).write_error,
        Some(LinePatchError::Stale.to_string())
    );
    assert_eq!(repo.index_file("a.txt"), b"one\ntwo\n", "nothing staged");
}

#[test]
fn applying_no_selected_lines_is_refused_as_empty() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\n")], "base");
    repo.write("a.txt", "one\ntwo\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    // A context line can't be selected.
    select_lines(&store, repo_id, &[new(1)]);
    let state = apply(&store, repo_id, LineAction::Stage);

    assert_eq!(
        active(&state).write_error,
        Some(LinePatchError::Empty.to_string())
    );
    assert_eq!(repo.index_file("a.txt"), b"one\n");
}

#[test]
fn a_partly_staged_new_file_can_be_unstaged_line_by_line() {
    let repo = TestRepo::with_commit("repo");
    repo.write("new.txt", "one\ntwo\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Untracked, "new.txt");

    select_lines(&store, repo_id, &[new(2)]);
    let state = apply(&store, repo_id, LineAction::Stage);
    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.index_file("new.txt"), b"two\n");

    repo.git(&["add", "new.txt"]);
    store.dispatch(Msg::RefreshStatus(repo_id));
    wait_settled(&store);
    show(&store, repo_id, ChangeGroup::Staged, "new.txt");
    select_lines(&store, repo_id, &[new(1)]);
    let state = apply(&store, repo_id, LineAction::Unstage);
    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.index_file("new.txt"), b"two\n");
}

#[test]
fn a_line_only_the_two_column_alignment_calls_changed_is_refused_as_unmapped() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\nb\nc\nd\n")], "base");
    repo.write("a.txt", "a\nc\nb\nd\n");
    let (store, repo_id) = open(&repo);
    store.dispatch(Msg::SetDiffOptions(DiffOptions {
        layout: DiffLayout::TwoColumn,
        ..DiffOptions::default()
    }));
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    // The columns show `c` moved up as added; git shows `b` moved down.
    select_lines(&store, repo_id, &[new(2)]);
    let state = apply(&store, repo_id, LineAction::Stage);

    assert_eq!(
        active(&state).write_error,
        Some(LinePatchError::Unmapped.to_string())
    );
    assert_eq!(repo.index_file("a.txt"), b"a\nb\nc\nd\n");
}

#[test]
fn discarding_the_removal_of_a_last_line_without_a_newline_restores_it_with_one() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\nlast")], "base");
    repo.write("a.txt", "a\nlast\nnew\n");
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    // git shows `-last`, `\ No newline`, `+last`, `+new`; undo only the
    // removal, so the old `last` comes back before the lines kept.
    select_lines(&store, repo_id, &[old(2)]);
    apply(&store, repo_id, LineAction::Discard);
    store.dispatch(Msg::ConfirmDiscard(repo_id));
    let state = wait_settled(&store);

    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.work_file("a.txt"), b"a\nlast\nlast\nnew\n");
}

#[test]
fn unstaging_one_line_of_a_staged_rename_keeps_the_rename() {
    let repo = TestRepo::init("repo");
    let base = numbered(10);
    repo.commit_files(&[("old.txt", &base)], "base");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", &base.replace("line 5\n", "LINE 5\n"));
    repo.git(&["add", "new.txt"]);
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Staged, "new.txt");

    select_lines(&store, repo_id, &[new(5)]);
    let state = apply(&store, repo_id, LineAction::Unstage);

    assert_eq!(active(&state).write_error, None);
    // Still renamed; the edit's removal stays staged, so not quite 100%.
    let staged = repo.git(&["diff", "--cached", "--name-status", "-M"]);
    assert!(
        staged.starts_with('R') && staged.ends_with("\told.txt\tnew.txt"),
        "still renamed: {staged}"
    );
    assert_eq!(
        String::from_utf8(repo.index_file("new.txt")).unwrap(),
        base.replace("line 5\n", "")
    );
}

#[test]
fn discarding_every_line_of_a_file_added_with_intent_leaves_it_empty() {
    let repo = TestRepo::with_commit("repo");
    repo.write("new.txt", "one\ntwo\n");
    repo.git(&["add", "--intent-to-add", "new.txt"]);
    let (store, repo_id) = open(&repo);
    show(&store, repo_id, ChangeGroup::Unstaged, "new.txt");

    store.dispatch(Msg::ApplyHunk {
        repo_id,
        hunk: 0,
        action: LineAction::Discard,
    });
    store.dispatch(Msg::ConfirmDiscard(repo_id));
    let state = wait_settled(&store);

    assert_eq!(active(&state).write_error, None);
    assert_eq!(repo.work_file("new.txt"), b"", "the file stays, emptied");
}
