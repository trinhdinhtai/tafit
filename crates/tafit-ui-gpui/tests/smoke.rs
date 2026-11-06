//! GPUI smoke test: a Window renders a Repository Tab. Logic is tested through
//! the store (tafit-state), not here.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::TestAppContext;
use tafit_core::{Head, RepositoryInfo};
use tafit_git_gix::GixBackend;
use tafit_state::{AppState, AppStore, Msg, reduce};
use tafit_ui_gpui::TafitView;

/// A snapshot with one opened Repository, built with the synchronous reducer.
fn state_with_open_repository() -> AppState {
    let mut state = AppState::default();
    reduce(&mut state, Msg::OpenRepo(PathBuf::from("/work/my-repo")));
    let repo_id = state.active_repo.expect("OpenRepo activates the new tab");
    reduce(
        &mut state,
        Msg::RepoOpened {
            repo_id,
            result: Ok(RepositoryInfo {
                name: "my-repo".to_owned(),
                head: Head::Branch("main".to_owned()),
            }),
        },
    );
    state
}

#[gpui::test]
fn window_renders_the_repository_tab(cx: &mut TestAppContext) {
    let (store, _events) = AppStore::with_state(
        Arc::new(GixBackend::default()),
        state_with_open_repository(),
    );

    // No events receiver: the live poller is off under GPUI's test scheduler.
    let (_view, cx) = cx.add_window_view(|_window, cx| TafitView::new(Arc::new(store), None, cx));
    cx.run_until_parked();

    assert!(cx.debug_bounds("repository-tab").is_some(), "tab bar entry");
    assert!(
        cx.debug_bounds("repository-name").is_some(),
        "Repository name"
    );
    assert!(cx.debug_bounds("repository-head").is_some(), "HEAD line");
}
