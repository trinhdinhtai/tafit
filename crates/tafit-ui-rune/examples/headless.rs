//! Opens a Repository, waits until it has loaded, then runs the UI with
//! Rune's own flags, so it can be checked without a window:
//!
//! `cargo run -p tafit-ui-rune --example headless -- --snap shot.png`
//!
//! The Repository is `TAFIT_REPO`, or the current directory. `TAFIT_SELECT`
//! (`staged:path`, `unstaged:path` or `untracked:path`) selects a file and
//! waits for its diff too. `TAFIT_VIEW=history` shows the history instead
//! of the changes.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tafit_git_gix::GixBackend;
use tafit_state::{
    AppStore, ChangeGroup, FileRef, Loadable, Msg, SelectMode, TabContent, WindowId,
};

fn main() {
    let path = std::env::var_os("TAFIT_REPO").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let path = std::path::absolute(path).expect("an absolute path");
    let (store, events) = AppStore::new(Arc::new(GixBackend::default()));
    store.dispatch(Msg::DetectGit);
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path,
    });
    wait(&store, |repo| {
        matches!(repo.open, Loadable::Ready(_)) && matches!(repo.status, Loadable::Ready(_))
    });
    if let Some(file) = std::env::var("TAFIT_SELECT")
        .ok()
        .and_then(|f| file_ref(&f))
    {
        let repo_id = store
            .snapshot()
            .active_repo()
            .expect("the opened Repository");
        store.dispatch(Msg::Select {
            repo_id,
            file,
            mode: SelectMode::Only,
        });
        wait(&store, |repo| {
            repo.diff
                .as_ref()
                .is_some_and(|diff| matches!(diff.diff, Loadable::Ready(_)))
        });
    }
    if std::env::var("TAFIT_VIEW").is_ok_and(|view| view == "history") {
        let repo_id = store
            .snapshot()
            .active_repo()
            .expect("the opened Repository");
        store.dispatch(Msg::ShowTabContent {
            repo_id,
            content: TabContent::History,
        });
        wait(&store, |repo| {
            repo.content == TabContent::History && matches!(repo.history.index, Loadable::Ready(_))
        });
    }
    tafit_ui_rune::run(Arc::new(store), events);
}

/// Waits up to 20 s for the Active Repository to be `done` and git found.
fn wait(store: &AppStore, done: impl Fn(&tafit_state::RepoState) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let state = store.snapshot();
        if state.active().is_some_and(&done) && !matches!(state.git, Loadable::Loading) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn file_ref(arg: &str) -> Option<FileRef> {
    let (group, path) = arg.split_once(':')?;
    let group = match group {
        "staged" => ChangeGroup::Staged,
        "unstaged" => ChangeGroup::Unstaged,
        "untracked" => ChangeGroup::Untracked,
        _ => return None,
    };
    Some(FileRef {
        group,
        path: path.to_owned(),
    })
}
