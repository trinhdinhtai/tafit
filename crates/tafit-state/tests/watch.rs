//! Changes made outside the app, by an editor or the git CLI, showing up in
//! the store on their own.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use support::{TestRepo, TestStore};
use tafit_core::{Head, RepositoryStatus};
use tafit_state::{AppState, Loadable, Msg, RepoId, RepoState, Watch, WatchOptions, WindowId};

/// Longer than a batch takes to land, with room to spare.
const SETTLE: Duration = Duration::from_millis(1500);

fn repo_in(state: &AppState, repo_id: RepoId) -> &RepoState {
    state.repo(repo_id).expect("the Repository is open")
}

fn status(state: &AppState, repo_id: RepoId) -> Option<Arc<RepositoryStatus>> {
    match &repo_in(state, repo_id).status {
        Loadable::Ready(status) => Some(Arc::clone(status)),
        _ => None,
    }
}

fn untracked(state: &AppState, repo_id: RepoId) -> Vec<String> {
    status(state, repo_id)
        .map(|status| {
            status
                .untracked
                .iter()
                .map(|file| file.path.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Opens `path` and waits until its change list is shown and it is watched.
fn open_watched(store: &TestStore, path: &Path) -> RepoId {
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: path.to_owned(),
    });
    store.flush();
    let state = store.wait_until(|state| {
        state.active().is_some_and(|repo| {
            repo.watch == Watch::Watching
                && matches!(repo.status, Loadable::Ready(_))
                && !repo.status_refreshing
        })
    });
    state
        .active_repo()
        .expect("the opened Repository is active")
}

#[test]
fn a_file_written_outside_the_app_shows_up() {
    let repo = TestRepo::with_commit("repo");
    let store = TestStore::new();
    let repo_id = open_watched(&store, repo.path());

    repo.write("notes.txt", "from an editor\n");

    store.wait_until(|state| untracked(state, repo_id) == ["notes.txt"]);
}

#[test]
fn a_commit_made_with_the_git_cli_shows_up() {
    let repo = TestRepo::with_commit("repo");
    repo.write("a.txt", "a\n");
    repo.git(&["add", "a.txt"]);
    let store = TestStore::new();
    let repo_id = open_watched(&store, repo.path());
    assert!(
        !status(&store.store.snapshot(), repo_id)
            .unwrap()
            .staged
            .is_empty()
    );

    repo.git(&["switch", "--quiet", "-c", "feature"]);
    repo.commit("from the cli");

    store.wait_until(|state| {
        let repo = repo_in(state, repo_id);
        matches!(&repo.open, Loadable::Ready(info) if info.head == Head::Branch("feature".to_owned()))
            && status(state, repo_id).is_some_and(|status| status.is_clean())
    });
}

#[test]
fn a_detached_head_moving_shows_up() {
    let repo = TestRepo::with_commit("repo");
    repo.git(&["switch", "--quiet", "--detach"]);
    let store = TestStore::new();
    let repo_id = open_watched(&store, repo.path());

    repo.commit("moves HEAD");
    let head = repo.git(&["rev-parse", "--short=7", "HEAD"]);

    store.wait_until(|state| {
        matches!(&repo_in(state, repo_id).open, Loadable::Ready(info) if info.head == Head::Detached(head.clone()))
    });
}

#[test]
fn changes_in_an_ignored_folder_read_nothing() {
    let repo = TestRepo::with_commit("repo");
    repo.commit_files(&[(".gitignore", "target/\n")], "ignore target");
    std::fs::create_dir(repo.path().join("target")).unwrap();
    let store = TestStore::new();
    let repo_id = open_watched(&store, repo.path());
    let before = status(&store.store.snapshot(), repo_id).unwrap();

    repo.write("target/debug/out.o", "built\n");
    repo.write("target/debug/deep/more.o", "built\n");
    std::thread::sleep(SETTLE);

    let after = status(&store.store.snapshot(), repo_id).unwrap();
    assert!(
        Arc::ptr_eq(&before, &after),
        "the change list was read again"
    );
    // The watch still works for files that aren't ignored.
    repo.write("seen.txt", "x\n");
    store.wait_until(|state| untracked(state, repo_id) == ["seen.txt"]);
}

#[test]
fn a_repository_left_is_read_again_when_its_tab_is_selected() {
    let first = TestRepo::with_commit("first");
    let second = TestRepo::with_commit("second");
    let store = TestStore::new();
    let first_id = open_watched(&store, first.path());
    let second_id = open_watched(&store, second.path());
    assert_eq!(repo_in(&store.store.snapshot(), first_id).watch, Watch::Off);

    first.write("while-away.txt", "x\n");
    std::thread::sleep(SETTLE);
    assert!(untracked(&store.store.snapshot(), first_id).is_empty());

    store.dispatch(Msg::ActivateRepo(first_id));

    let state = store.wait_until(|state| untracked(state, first_id) == ["while-away.txt"]);
    assert_eq!(repo_in(&state, second_id).watch, Watch::Off);
}

#[test]
fn per_folder_watches_follow_new_folders() {
    let repo = TestRepo::with_commit("repo");
    let store = TestStore::with_watch_options(WatchOptions {
        per_folder: true,
        ..WatchOptions::default()
    });
    let repo_id = open_watched(&store, repo.path());

    std::fs::create_dir(repo.path().join("new")).unwrap();
    store.wait_until(|state| untracked(state, repo_id).is_empty());
    std::thread::sleep(SETTLE);
    repo.write("new/inner/file.txt", "x\n");

    store.wait_until(|state| untracked(state, repo_id) == ["new/"]);
    repo.git(&["add", "new"]);
    repo.write("new/inner/file.txt", "changed\n");

    store.wait_until(|state| {
        status(state, repo_id).is_some_and(|status| {
            status
                .unstaged
                .iter()
                .any(|file| file.path == "new/inner/file.txt")
        })
    });
}

#[test]
fn too_many_folders_degrade_the_watch_to_rereading_now_and_then() {
    let repo = TestRepo::with_commit("repo");
    for ix in 0..5 {
        std::fs::create_dir(repo.path().join(format!("folder-{ix}"))).unwrap();
    }
    let store = TestStore::with_watch_options(WatchOptions {
        per_folder: true,
        max_folders: 3,
        idle_tick: Duration::from_millis(300),
        ..WatchOptions::default()
    });
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: repo.path().to_owned(),
    });
    let state = store.wait_until(|state| {
        state
            .active()
            .is_some_and(|repo| matches!(repo.watch, Watch::Degraded(_)))
    });
    let repo_id = state.active_repo().unwrap();

    // Written where no watch is, it shows up on the next tick anyway.
    let unwatched = (0..5)
        .map(|ix| format!("folder-{ix}/file.txt"))
        .collect::<Vec<_>>();
    for path in &unwatched {
        repo.write(path, "x\n");
    }

    store.wait_until(|state| untracked(state, repo_id).len() == 5);
}
