mod support;

use std::path::Path;

use support::{TestRepo, TestStore};
use tafit_core::{ChangeKind, FileChange, RepositoryStatus};
use tafit_state::{Loadable, Msg, WindowId};

/// Opens `path` in a fresh store and returns the Repository's first change list.
fn status(path: &Path) -> RepositoryStatus {
    let store = TestStore::new();
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: path.to_owned(),
    });
    let state = store.wait_until(|state| {
        state
            .active()
            .is_some_and(|repo| !matches!(repo.status, Loadable::Loading))
    });
    match &state
        .active()
        .expect("the opened Repository is active")
        .status
    {
        Loadable::Ready(status) => (**status).clone(),
        other => panic!("expected the change list to load, got {other:?}"),
    }
}

fn change(path: &str, kind: ChangeKind) -> FileChange {
    FileChange {
        path: path.to_owned(),
        kind,
    }
}

#[test]
fn a_new_file_is_untracked() {
    let repo = TestRepo::with_commit("repo");
    repo.write("new.txt", "hello\n");

    let status = status(repo.path());

    assert_eq!(status.untracked, [change("new.txt", ChangeKind::Added)]);
    assert!(status.staged.is_empty() && status.unstaged.is_empty());
}

#[test]
fn a_clean_repository_has_no_changes() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\n")], "add a");

    assert!(status(repo.path()).is_clean());
}

#[test]
fn an_edited_file_is_modified_but_not_staged() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\n")], "add a");
    repo.write("a.txt", "changed\n");

    let status = status(repo.path());

    assert_eq!(status.unstaged, [change("a.txt", ChangeKind::Modified)]);
    assert!(status.staged.is_empty() && status.untracked.is_empty());
}

#[test]
fn a_file_removed_from_disk_is_deleted_but_not_staged() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "a\n")], "add a");
    repo.delete("a.txt");

    let status = status(repo.path());

    assert_eq!(status.unstaged, [change("a.txt", ChangeKind::Deleted)]);
    assert!(status.staged.is_empty());
}

#[test]
fn staged_additions_edits_and_deletions_are_staged() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("edit.txt", "a\n"), ("gone.txt", "b\n")], "base");
    repo.write("edit.txt", "changed\n");
    repo.write("new.txt", "new\n");
    repo.git(&["add", "edit.txt", "new.txt"]);
    repo.git(&["rm", "--quiet", "gone.txt"]);

    let status = status(repo.path());

    assert_eq!(
        status.staged,
        [
            change("edit.txt", ChangeKind::Modified),
            change("gone.txt", ChangeKind::Deleted),
            change("new.txt", ChangeKind::Added),
        ]
    );
    assert!(status.unstaged.is_empty() && status.untracked.is_empty());
}

#[test]
fn a_moved_file_is_staged_as_a_rename() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("old.txt", "some content that stays the same\n")], "base");
    std::fs::create_dir(repo.path().join("docs")).expect("create docs folder");
    repo.git(&["mv", "old.txt", "docs/new.txt"]);

    let status = status(repo.path());

    assert_eq!(
        status.staged,
        [change(
            "docs/new.txt",
            ChangeKind::Renamed {
                from: "old.txt".to_owned()
            }
        )]
    );
}

#[test]
fn a_partly_staged_file_is_both_staged_and_unstaged() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "one\n")], "base");
    repo.write("a.txt", "one\ntwo\n");
    repo.git(&["add", "a.txt"]);
    repo.write("a.txt", "one\ntwo\nthree\n");

    let status = status(repo.path());

    assert_eq!(status.staged, [change("a.txt", ChangeKind::Modified)]);
    assert_eq!(status.unstaged, [change("a.txt", ChangeKind::Modified)]);
}

#[test]
fn a_new_folder_is_untracked_as_a_whole_and_ignored_files_are_left_out() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[(".gitignore", "build/\n")], "ignore build");
    repo.write("assets/a.png", "a");
    repo.write("assets/b.png", "b");
    repo.write("build/out.o", "o");

    let status = status(repo.path());

    assert_eq!(status.untracked, [change("assets/", ChangeKind::Added)]);
}

#[test]
fn files_staged_before_the_first_commit_are_added() {
    let repo = TestRepo::init("fresh");
    repo.write("a.txt", "a\n");
    repo.git(&["add", "a.txt"]);

    let status = status(repo.path());

    assert_eq!(status.staged, [change("a.txt", ChangeKind::Added)]);
}

#[test]
fn a_bare_repository_has_no_changes() {
    let repo = TestRepo::init_bare("server.git");

    assert!(status(repo.path()).is_clean());
}

#[test]
fn refreshing_picks_up_changes_made_since_the_last_read() {
    let repo = TestRepo::with_commit("repo");
    let store = TestStore::new();
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: repo.path().to_owned(),
    });
    let opened = store.wait_until(|state| {
        state
            .active()
            .is_some_and(|repo| matches!(repo.status, Loadable::Ready(_)))
    });
    let repo_id = opened
        .active_repo()
        .expect("the opened Repository is active");
    repo.write("later.txt", "written after opening\n");

    store.dispatch(Msg::RefreshStatus(repo_id));

    store.wait_until(|state| {
        state.active().is_some_and(|repo| match &repo.status {
            Loadable::Ready(status) => status.untracked == [change("later.txt", ChangeKind::Added)],
            _ => false,
        })
    });
}

#[test]
fn a_file_added_with_intent_to_add_is_a_new_unstaged_file() {
    let repo = TestRepo::with_commit("repo");
    repo.write("planned.txt", "later\n");
    repo.git(&["add", "--intent-to-add", "planned.txt"]);

    let status = status(repo.path());

    assert_eq!(status.unstaged, [change("planned.txt", ChangeKind::Added)]);
    assert!(status.staged.is_empty() && status.untracked.is_empty());
}

#[test]
fn a_conflicted_file_is_listed_once_as_unstaged() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "base\n")], "base");
    repo.git(&["switch", "--quiet", "-c", "other"]);
    repo.commit_files(&[("a.txt", "theirs\n")], "theirs");
    repo.git(&["switch", "--quiet", "main"]);
    repo.commit_files(&[("a.txt", "ours\n")], "ours");
    assert!(
        !repo.try_git(&["merge", "--quiet", "other"]),
        "the merge conflicts"
    );

    let status = status(repo.path());

    assert_eq!(status.unstaged, [change("a.txt", ChangeKind::Modified)]);
    assert!(status.staged.is_empty(), "staged: {:?}", status.staged);
}

#[test]
fn a_merge_left_to_conclude_is_in_progress() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.txt", "base\n")], "base");
    repo.git(&["switch", "--quiet", "-c", "other"]);
    repo.commit_files(&[("b.txt", "theirs\n")], "theirs");
    repo.git(&["switch", "--quiet", "main"]);
    repo.git(&["merge", "--quiet", "--no-ff", "--no-commit", "other"]);

    assert!(status(repo.path()).merge_in_progress);
}

#[test]
fn no_merge_is_in_progress_in_a_plain_repository() {
    let repo = TestRepo::with_commit("repo");

    assert!(!status(repo.path()).merge_in_progress);
}
