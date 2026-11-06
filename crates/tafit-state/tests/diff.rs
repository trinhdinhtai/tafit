//! The Line Diff of the selected file, read through the store from real repos.

mod support;

use std::sync::Arc;

use support::{TestRepo, TestStore};
use tafit_core::{DiffLine, FileDiff, LineKind};
use tafit_state::{AppState, ChangeGroup, FileRef, Loadable, Msg, RepoId, SelectMode, WindowId};

/// Opens `repo` in a fresh store once its change list has loaded.
fn open(repo: &TestRepo) -> (TestStore, RepoId) {
    let store = TestStore::new();
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: repo.path().to_owned(),
    });
    let state = store.wait_until(|state| {
        state
            .active()
            .is_some_and(|repo| matches!(repo.status, Loadable::Ready(_)))
    });
    (store, state.active_repo().expect("the opened Repository"))
}

/// Selects only `path` in `group` and waits for its diff to load.
fn diff_of(store: &TestStore, repo_id: RepoId, group: ChangeGroup, path: &str) -> Arc<FileDiff> {
    let file = FileRef {
        group,
        path: path.to_owned(),
    };
    store.dispatch(Msg::Select {
        repo_id,
        file: file.clone(),
        mode: SelectMode::Only,
    });
    store.flush();
    let state = store.wait_until(|state| loaded_diff(state, &file).is_some());
    loaded_diff(&state, &file).expect("waited for it")
}

fn loaded_diff(state: &AppState, file: &FileRef) -> Option<Arc<FileDiff>> {
    let diff = state.active()?.diff.as_ref()?;
    match &diff.diff {
        Loadable::Ready(loaded) if diff.file == *file => Some(Arc::clone(loaded)),
        Loadable::Error(error) => panic!("the diff failed: {error}"),
        _ => None,
    }
}

/// Each line as `marker old new text`, e.g. `+ - 2 two`, for comparing.
fn lines(diff: &FileDiff) -> Vec<String> {
    let FileDiff::Text(hunks) = diff else {
        panic!("expected a text diff, got {diff:?}");
    };
    let number = |n: Option<u32>| n.map_or("-".to_owned(), |n| n.to_string());
    hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .map(|line: &DiffLine| {
            let marker = match line.kind {
                LineKind::Context => ' ',
                LineKind::Added => '+',
                LineKind::Removed => '-',
            };
            format!(
                "{marker} {} {} {}",
                number(line.old_line),
                number(line.new_line),
                line.text
            )
        })
        .collect()
}

#[test]
fn an_edited_line_shows_as_removed_then_added_with_line_numbers() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\ntwo\nthree\n")], "base");
    repo.write("a.txt", "one\nTWO\nthree\nfour\n");
    let (store, repo_id) = open(&repo);

    let diff = diff_of(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    assert_eq!(
        lines(&diff),
        [
            "  1 1 one",
            "- 2 - two",
            "+ - 2 TWO",
            "  3 3 three",
            "+ - 4 four"
        ]
    );
}

#[test]
fn a_file_staged_in_part_has_one_diff_staged_and_another_unstaged() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\n")], "base");
    repo.write("a.txt", "one\nstaged\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", "one\nstaged\nunstaged\n");
    let (store, repo_id) = open(&repo);

    let staged = diff_of(&store, repo_id, ChangeGroup::Staged, "a.txt");
    let unstaged = diff_of(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    assert_eq!(lines(&staged), ["  1 1 one", "+ - 2 staged"]);
    assert_eq!(
        lines(&unstaged),
        ["  1 1 one", "  2 2 staged", "+ - 3 unstaged"]
    );
}

#[test]
fn a_deleted_file_shows_every_line_removed() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("gone.txt", "a\nb\n")], "base");
    repo.delete("gone.txt");
    let (store, repo_id) = open(&repo);

    let diff = diff_of(&store, repo_id, ChangeGroup::Unstaged, "gone.txt");

    assert_eq!(lines(&diff), ["- 1 - a", "- 2 - b"]);
}

#[test]
fn an_untracked_file_shows_every_line_added() {
    let repo = TestRepo::init("repo");
    repo.write("new.txt", "a\nb");
    let (store, repo_id) = open(&repo);

    let diff = diff_of(&store, repo_id, ChangeGroup::Untracked, "new.txt");

    assert_eq!(lines(&diff), ["+ - 1 a", "+ - 2 b"]);
    let FileDiff::Text(hunks) = &*diff else {
        unreachable!("checked by lines")
    };
    let last = hunks[0].lines.last().expect("two lines");
    assert!(last.no_newline_at_end, "the file ends without a newline");
}

#[test]
fn a_binary_file_is_reported_as_binary() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("image.bin", "x\0y")], "base");
    repo.write("image.bin", "x\0z");
    let (store, repo_id) = open(&repo);

    let diff = diff_of(&store, repo_id, ChangeGroup::Unstaged, "image.bin");

    assert_eq!(*diff, FileDiff::Binary);
}

#[test]
fn a_file_with_a_unicode_name_has_its_diff() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("tài liệu/ghi chú.txt", "cũ\n")], "base");
    repo.write("tài liệu/ghi chú.txt", "mới\n");
    let (store, repo_id) = open(&repo);

    let diff = diff_of(
        &store,
        repo_id,
        ChangeGroup::Unstaged,
        "tài liệu/ghi chú.txt",
    );

    assert_eq!(lines(&diff), ["- 1 - cũ", "+ - 1 mới"]);
}

#[test]
fn the_users_diff_config_does_not_change_the_diff() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("ghi chú.txt", "one\n\nthree\n")], "base");
    repo.write("ghi chú.txt", "one\n\nTHREE\n");
    for (key, value) in [
        ("diff.external", "false"),
        ("color.ui", "always"),
        ("color.diff", "always"),
        ("core.quotepath", "true"),
        ("diff.noprefix", "true"),
        ("diff.mnemonicPrefix", "true"),
        ("diff.suppressBlankEmpty", "true"),
    ] {
        repo.git(&["config", key, value]);
    }
    let (store, repo_id) = open(&repo);

    let diff = diff_of(&store, repo_id, ChangeGroup::Unstaged, "ghi chú.txt");

    assert_eq!(
        lines(&diff),
        ["  1 1 one", "  2 2 ", "- 3 - three", "+ - 3 THREE"]
    );
}

#[test]
fn selecting_a_second_file_hides_the_diff() {
    let repo = TestRepo::init("repo");
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let (store, repo_id) = open(&repo);
    diff_of(&store, repo_id, ChangeGroup::Untracked, "a.txt");

    store.dispatch(Msg::Select {
        repo_id,
        file: FileRef {
            group: ChangeGroup::Untracked,
            path: "b.txt".to_owned(),
        },
        mode: SelectMode::Toggle,
    });
    store.flush();

    let state = store.store.snapshot();
    assert!(state.active().expect("open").diff.is_none());
}

#[test]
fn a_refresh_rereads_the_diff_of_a_file_that_changed() {
    let repo = TestRepo::init("repo");
    repo.write("a.txt", "a\n");
    let (store, repo_id) = open(&repo);
    diff_of(&store, repo_id, ChangeGroup::Untracked, "a.txt");

    repo.write("a.txt", "a\nb\n");
    store.dispatch(Msg::RefreshStatus(repo_id));
    store.flush();

    store.wait_until(|state| {
        state
            .active()
            .and_then(|repo| repo.diff.as_ref())
            .is_some_and(|diff| match &diff.diff {
                Loadable::Ready(diff) => lines(diff).len() == 2,
                _ => false,
            })
    });
}

#[test]
fn adding_a_newline_at_the_end_keeps_every_line_after_the_marker() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a")], "base");
    repo.write("a.txt", "a\nb\n");
    let (store, repo_id) = open(&repo);

    let diff = diff_of(&store, repo_id, ChangeGroup::Unstaged, "a.txt");

    assert_eq!(lines(&diff), ["- 1 - a", "+ - 1 a", "+ - 2 b"]);
    let FileDiff::Text(hunks) = &*diff else {
        unreachable!("checked by lines")
    };
    assert!(
        hunks[0].lines[0].no_newline_at_end,
        "the old side ended without a newline"
    );
}
