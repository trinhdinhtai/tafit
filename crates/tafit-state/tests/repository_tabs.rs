//! Repository Tabs and Windows: opening, closing, reordering and switching
//! tabs, Windows with their own tabs, and the recent and pinned Repositories.

mod support;

use std::path::{Path, PathBuf};

use support::{TestDir, TestRepo, TestStore};
use tafit_state::{AppState, Loadable, Msg, Page, RECENT_LIMIT, RepoId, WindowId};

/// The paths of `window`'s Repository Tabs, in tab order.
fn tab_paths(state: &AppState, window: WindowId) -> Vec<&Path> {
    state.tabs(window).map(|repo| repo.path.as_path()).collect()
}

/// Opens `repo` in `window` and waits until it has loaded.
fn open(store: &TestStore, window: WindowId, repo: &TestRepo) -> RepoId {
    store.dispatch(Msg::OpenRepo {
        window,
        path: repo.path().to_owned(),
    });
    store.flush();
    let state = store.wait_until(|state| {
        state.active_in(window).is_some_and(|active| {
            active.path == repo.path() && matches!(active.open, Loadable::Ready(_))
        })
    });
    state
        .active_in(window)
        .expect("the opened Repository is active")
        .id
}

#[test]
fn opening_another_repository_adds_a_tab_and_makes_it_active() {
    let (first, second) = (
        TestRepo::with_commit("first"),
        TestRepo::with_commit("second"),
    );
    let store = TestStore::new();

    open(&store, WindowId::FIRST, &first);
    let second_id = open(&store, WindowId::FIRST, &second);

    let state = store.store.snapshot();
    assert_eq!(
        tab_paths(&state, WindowId::FIRST),
        [first.path(), second.path()]
    );
    assert_eq!(
        state.window(WindowId::FIRST).unwrap().active_repo,
        Some(second_id)
    );
}

#[test]
fn opening_an_open_repository_again_activates_its_tab_instead_of_adding_one() {
    let (first, second) = (
        TestRepo::with_commit("first"),
        TestRepo::with_commit("second"),
    );
    let store = TestStore::new();
    let first_id = open(&store, WindowId::FIRST, &first);
    open(&store, WindowId::FIRST, &second);

    let again = open(&store, WindowId::FIRST, &first);

    let state = store.store.snapshot();
    assert_eq!(again, first_id, "the same Repository Tab");
    assert_eq!(
        tab_paths(&state, WindowId::FIRST),
        [first.path(), second.path()]
    );
    assert_eq!(state.repos.len(), 2, "no second copy of the Repository");
}

#[test]
fn selecting_a_tab_makes_its_repository_active() {
    let (first, second) = (
        TestRepo::with_commit("first"),
        TestRepo::with_commit("second"),
    );
    let store = TestStore::new();
    let first_id = open(&store, WindowId::FIRST, &first);
    open(&store, WindowId::FIRST, &second);

    store.dispatch(Msg::ActivateRepo(first_id));
    store.flush();

    let state = store.store.snapshot();
    assert_eq!(state.active_repo(), Some(first_id));
    assert_eq!(
        tab_paths(&state, WindowId::FIRST),
        [first.path(), second.path()],
        "selecting doesn't reorder"
    );
}

#[test]
fn closing_the_active_tab_activates_the_one_after_it() {
    let repos = ["a", "b", "c"].map(TestRepo::with_commit);
    let store = TestStore::new();
    let ids = repos
        .each_ref()
        .map(|repo| open(&store, WindowId::FIRST, repo));
    store.dispatch(Msg::ActivateRepo(ids[1]));

    store.dispatch(Msg::CloseRepo(ids[1]));
    store.flush();

    let state = store.store.snapshot();
    assert_eq!(
        tab_paths(&state, WindowId::FIRST),
        [repos[0].path(), repos[2].path()]
    );
    assert_eq!(state.active_repo(), Some(ids[2]));
    assert!(state.repo(ids[1]).is_none(), "the Repository is closed");
}

#[test]
fn closing_the_last_tab_activates_the_one_before_it() {
    let repos = ["a", "b"].map(TestRepo::with_commit);
    let store = TestStore::new();
    let ids = repos
        .each_ref()
        .map(|repo| open(&store, WindowId::FIRST, repo));

    store.dispatch(Msg::CloseRepo(ids[1]));
    store.flush();

    assert_eq!(store.store.snapshot().active_repo(), Some(ids[0]));
}

#[test]
fn closing_an_inactive_tab_keeps_the_active_one() {
    let repos = ["a", "b"].map(TestRepo::with_commit);
    let store = TestStore::new();
    let ids = repos
        .each_ref()
        .map(|repo| open(&store, WindowId::FIRST, repo));

    store.dispatch(Msg::CloseRepo(ids[0]));
    store.flush();

    let state = store.store.snapshot();
    assert_eq!(state.active_repo(), Some(ids[1]));
    assert_eq!(tab_paths(&state, WindowId::FIRST), [repos[1].path()]);
}

#[test]
fn a_closed_repository_opens_again_in_a_new_tab() {
    let repo = TestRepo::with_commit("repo");
    let store = TestStore::new();
    let closed = open(&store, WindowId::FIRST, &repo);
    store.dispatch(Msg::CloseRepo(closed));
    store.flush();
    assert!(
        store.store.snapshot().active_repo().is_none(),
        "no tab left"
    );

    let reopened = open(&store, WindowId::FIRST, &repo);

    assert_ne!(reopened, closed);
    assert_eq!(
        tab_paths(&store.store.snapshot(), WindowId::FIRST),
        [repo.path()]
    );
}

#[test]
fn a_tab_moves_to_where_it_is_dropped() {
    let repos = ["a", "b", "c"].map(TestRepo::with_commit);
    let store = TestStore::new();
    let ids = repos
        .each_ref()
        .map(|repo| open(&store, WindowId::FIRST, repo));
    let [a, b, c] = repos.each_ref().map(TestRepo::path);

    store.dispatch(Msg::MoveTab {
        repo_id: ids[2],
        to: 0,
    });
    store.flush();
    assert_eq!(
        tab_paths(&store.store.snapshot(), WindowId::FIRST),
        [c, a, b]
    );

    store.dispatch(Msg::MoveTab {
        repo_id: ids[2],
        to: 2,
    });
    store.flush();
    let state = store.store.snapshot();
    assert_eq!(tab_paths(&state, WindowId::FIRST), [a, b, c]);
    assert_eq!(
        state.active_repo(),
        Some(ids[2]),
        "moving keeps the active tab"
    );
}

#[test]
fn a_tab_dropped_past_the_end_moves_to_the_end() {
    let repos = ["a", "b"].map(TestRepo::with_commit);
    let store = TestStore::new();
    let ids = repos
        .each_ref()
        .map(|repo| open(&store, WindowId::FIRST, repo));

    store.dispatch(Msg::MoveTab {
        repo_id: ids[0],
        to: 9,
    });
    store.flush();

    assert_eq!(
        tab_paths(&store.store.snapshot(), WindowId::FIRST),
        [repos[1].path(), repos[0].path()]
    );
}

/// Opens a new Window and returns its id.
fn open_window(store: &TestStore) -> WindowId {
    let before = store.store.snapshot().windows.len();
    store.dispatch(Msg::OpenWindow);
    store.flush();
    let state = store.store.snapshot();
    assert_eq!(state.windows.len(), before + 1, "one more Window");
    state.windows.last().expect("the new Window").id
}

#[test]
fn a_new_window_has_its_own_tabs() {
    let (first, second) = (
        TestRepo::with_commit("first"),
        TestRepo::with_commit("second"),
    );
    let store = TestStore::new();
    let first_id = open(&store, WindowId::FIRST, &first);

    let window = open_window(&store);
    assert_eq!(
        tab_paths(&store.store.snapshot(), window),
        Vec::<&Path>::new()
    );
    let second_id = open(&store, window, &second);

    let state = store.store.snapshot();
    assert_eq!(tab_paths(&state, WindowId::FIRST), [first.path()]);
    assert_eq!(tab_paths(&state, window), [second.path()]);
    assert_eq!(state.active_in(WindowId::FIRST).unwrap().id, first_id);
    assert_eq!(state.active_in(window).unwrap().id, second_id);
    assert_eq!(state.focused_window, Some(window));
}

#[test]
fn opening_a_repository_open_in_another_window_brings_that_window_forward() {
    let repo = TestRepo::with_commit("repo");
    let store = TestStore::new();
    let repo_id = open(&store, WindowId::FIRST, &repo);
    let window = open_window(&store);

    store.dispatch(Msg::OpenRepo {
        window,
        path: repo.path().to_owned(),
    });
    store.flush();

    let state = store.store.snapshot();
    assert_eq!(
        tab_paths(&state, window),
        Vec::<&Path>::new(),
        "no second tab"
    );
    let first = state.window(WindowId::FIRST).unwrap();
    assert_eq!(first.active_repo, Some(repo_id));
    assert_eq!(
        first.raise_requests, 1,
        "the Window holding it comes forward"
    );
    assert_eq!(state.focused_window, Some(WindowId::FIRST));
}

#[test]
fn closing_a_window_closes_its_repositories() {
    let (first, second) = (
        TestRepo::with_commit("first"),
        TestRepo::with_commit("second"),
    );
    let store = TestStore::new();
    let first_id = open(&store, WindowId::FIRST, &first);
    let window = open_window(&store);
    let second_id = open(&store, window, &second);

    store.dispatch(Msg::CloseWindow(window));
    store.flush();

    let state = store.store.snapshot();
    assert!(state.window(window).is_none());
    assert!(state.repo(second_id).is_none(), "its Repository is closed");
    assert!(state.repo(first_id).is_some(), "the other Window's stays");
    assert_eq!(state.focused_window, Some(WindowId::FIRST));
}

#[test]
fn the_focused_window_follows_the_user() {
    let store = TestStore::new();
    open_window(&store);

    store.dispatch(Msg::FocusWindow(WindowId::FIRST));
    store.flush();

    assert_eq!(store.store.snapshot().focused_window, Some(WindowId::FIRST));
}

/// The Repositories offered for opening, as (path, pinned).
fn repository_list(store: &TestStore) -> Vec<(PathBuf, bool)> {
    store
        .store
        .snapshot()
        .repository_list()
        .map(|(path, pinned)| (path.to_owned(), pinned))
        .collect()
}

#[test]
fn opened_repositories_are_listed_most_recent_first() {
    let repos = ["a", "b"].map(TestRepo::with_commit);
    let store = TestStore::new();
    let ids = repos
        .each_ref()
        .map(|repo| open(&store, WindowId::FIRST, repo));
    store.dispatch(Msg::CloseRepo(ids[0]));
    open(&store, WindowId::FIRST, &repos[0]);

    assert_eq!(
        repository_list(&store),
        [
            (repos[0].path().to_owned(), false),
            (repos[1].path().to_owned(), false),
        ],
        "reopening moves it to the top, once"
    );
}

#[test]
fn a_folder_that_fails_to_open_is_not_listed() {
    let dir = TestDir::new("plain-folder");
    let store = TestStore::new();

    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: dir.path.clone(),
    });
    store.wait_until(|state| {
        matches!(
            state.active().map(|repo| &repo.open),
            Some(Loadable::Error(_))
        )
    });

    assert_eq!(repository_list(&store), []);
}

#[test]
fn only_the_latest_recent_repositories_are_kept() {
    let repos: Vec<_> = (0..=RECENT_LIMIT)
        .map(|ix| TestRepo::with_commit(&format!("repo-{ix}")))
        .collect();
    let store = TestStore::new();
    for repo in &repos {
        open(&store, WindowId::FIRST, repo);
    }

    let list = repository_list(&store);

    assert_eq!(list.len(), RECENT_LIMIT);
    assert_eq!(list[0].0, repos[RECENT_LIMIT].path());
    assert!(
        !list.iter().any(|(path, _)| path == repos[0].path()),
        "the oldest is dropped"
    );
}

#[test]
fn pinned_repositories_are_listed_first_and_stay_after_unpinning_as_recent() {
    let repos = ["a", "b", "c"].map(TestRepo::with_commit);
    let store = TestStore::new();
    for repo in &repos {
        open(&store, WindowId::FIRST, repo);
    }
    let [a, b, c] = repos.each_ref().map(|repo| repo.path().to_owned());

    store.dispatch(Msg::PinRepo(a.clone()));
    store.dispatch(Msg::PinRepo(b.clone()));
    store.dispatch(Msg::PinRepo(a.clone()));
    store.flush();
    assert_eq!(
        repository_list(&store),
        [(a.clone(), true), (b.clone(), true), (c.clone(), false)],
        "pin order, each once, then the rest"
    );

    store.dispatch(Msg::UnpinRepo(a.clone()));
    store.flush();
    assert_eq!(repository_list(&store), [(b, true), (c, false), (a, false)]);
}

#[test]
fn pinned_repositories_are_kept_past_the_recent_limit() {
    let pinned = TestRepo::with_commit("pinned");
    let store = TestStore::new();
    open(&store, WindowId::FIRST, &pinned);
    store.dispatch(Msg::PinRepo(pinned.path().to_owned()));
    let others: Vec<_> = (0..RECENT_LIMIT)
        .map(|ix| TestRepo::with_commit(&format!("repo-{ix}")))
        .collect();
    for repo in &others {
        open(&store, WindowId::FIRST, repo);
    }

    let list = repository_list(&store);

    assert_eq!(list[0], (pinned.path().to_owned(), true));
    assert_eq!(list.len(), RECENT_LIMIT + 1);
}

#[test]
fn each_window_shows_its_own_page_until_a_tab_is_selected() {
    let repo = TestRepo::with_commit("repo");
    let store = TestStore::new();
    let repo_id = open(&store, WindowId::FIRST, &repo);
    let window = open_window(&store);

    store.dispatch(Msg::ShowPage {
        window: WindowId::FIRST,
        page: Page::OpenRepository,
    });
    store.dispatch(Msg::ShowPage {
        window,
        page: Page::Settings,
    });
    store.flush();
    let state = store.store.snapshot();
    assert_eq!(
        state.window(WindowId::FIRST).unwrap().page,
        Page::OpenRepository
    );
    assert_eq!(state.window(window).unwrap().page, Page::Settings);

    store.dispatch(Msg::ActivateRepo(repo_id));
    store.flush();
    assert_eq!(
        store.store.snapshot().window(WindowId::FIRST).unwrap().page,
        Page::RepositoryTab
    );
}

#[test]
fn selecting_a_tab_in_another_window_focuses_that_window() {
    let repo = TestRepo::with_commit("repo");
    let store = TestStore::new();
    let repo_id = open(&store, WindowId::FIRST, &repo);
    open_window(&store);

    store.dispatch(Msg::ActivateRepo(repo_id));
    store.flush();

    let state = store.store.snapshot();
    assert_eq!(state.focused_window, Some(WindowId::FIRST));
    assert_eq!(state.active_repo(), Some(repo_id));
}
