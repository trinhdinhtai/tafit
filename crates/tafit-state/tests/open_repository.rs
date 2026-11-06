mod support;

use std::path::Path;

use support::{TestDir, TestRepo, TestStore};
use tafit_core::{Head, RepositoryInfo};
use tafit_state::{Loadable, Msg};

/// Opens `path` in a fresh store and returns the Repository Tab's settled state.
fn open(path: &Path) -> Loadable<RepositoryInfo> {
    let store = TestStore::new();
    store.dispatch(Msg::OpenRepo(path.to_owned()));
    let state = store.wait_until(|state| {
        state
            .active()
            .is_some_and(|repo| !matches!(repo.open, Loadable::Loading))
    });
    let active = state.active().expect("the opened Repository is active");
    assert_eq!(active.path, path);
    active.open.clone()
}

fn open_ok(path: &Path) -> RepositoryInfo {
    match open(path) {
        Loadable::Ready(info) => info,
        other => panic!("expected the Repository to open, got {other:?}"),
    }
}

#[test]
fn opening_a_repository_shows_its_name_and_head_branch() {
    let repo = TestRepo::with_commit("my-repo");

    let info = open_ok(repo.path());

    assert_eq!(info.name, "my-repo");
    assert_eq!(info.head, Head::Branch("main".to_owned()));
}

#[test]
fn opening_a_repository_on_a_detached_head_shows_the_commit() {
    let repo = TestRepo::with_commit("detached");
    repo.commit("second");
    let first = repo.git(&["rev-parse", "--short=7", "HEAD~1"]);
    repo.git(&["checkout", "--quiet", "--detach", "HEAD~1"]);

    let info = open_ok(repo.path());

    assert_eq!(info.head, Head::Detached(first));
}

#[test]
fn opening_a_folder_that_is_not_a_repository_reports_it_clearly() {
    let dir = TestDir::new("plain-folder");

    let Loadable::Error(message) = open(&dir.path) else {
        panic!("a plain folder must not open as a Repository");
    };

    assert!(
        message.starts_with("not a Git repository:") && message.contains("plain-folder"),
        "unclear error message: {message}"
    );
}

#[test]
fn opening_a_repository_without_commits_shows_the_unborn_branch() {
    let repo = TestRepo::init("fresh");

    let info = open_ok(repo.path());

    assert_eq!(info.head, Head::Unborn("main".to_owned()));
}

#[test]
fn opening_a_bare_repository_names_it_without_the_git_suffix() {
    let repo = TestRepo::init_bare("server.git");

    let info = open_ok(repo.path());

    assert_eq!(info.name, "server");
}

#[test]
fn opening_a_path_that_does_not_exist_says_so() {
    let dir = TestDir::new("parent");
    let missing = dir.path.join("missing");

    let Loadable::Error(message) = open(&missing) else {
        panic!("a missing path must not open as a Repository");
    };

    assert!(
        message.starts_with("path does not exist:"),
        "unclear error message: {message}"
    );
}
