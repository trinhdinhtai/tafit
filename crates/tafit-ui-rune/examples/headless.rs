//! Opens a Repository, waits until it has loaded, then runs the UI with
//! Rune's own flags, so it can be checked without a window:
//!
//! `cargo run -p tafit-ui-rune --example headless -- --snap shot.png`
//!
//! The Repository is `TAFIT_REPO`, or the current directory.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tafit_git_gix::GixBackend;
use tafit_state::{AppStore, Loadable, Msg, WindowId};

fn main() {
    let path = std::env::var_os("TAFIT_REPO").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let path = std::path::absolute(path).expect("an absolute path");
    let (store, events) = AppStore::new(Arc::new(GixBackend::default()));
    store.dispatch(Msg::DetectGit);
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path,
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let state = store.snapshot();
        let loaded = state.active().is_some_and(|repo| {
            matches!(repo.open, Loadable::Ready(_)) && matches!(repo.status, Loadable::Ready(_))
        });
        if loaded && !matches!(state.git, Loadable::Loading) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    tafit_ui_rune::run(Arc::new(store), events);
}
