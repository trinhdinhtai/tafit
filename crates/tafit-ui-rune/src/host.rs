//! Keeps Rune's windows in step with the store's: one Rune window per
//! `WindowState`. One thread follows the store's events and wakes every
//! window, which then reads the latest snapshot in its `poll`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use rune::{App, AppSpec, Tx, WindowHandle, WindowSpec};
use tafit_base::APP_NAME;
use tafit_state::{AppStore, Msg, StoreEvents, WindowId};

use crate::Ui;

/// What every window shares.
pub(crate) struct Shared {
    pub(crate) store: Arc<AppStore>,
    /// Wakes each window when the store changed.
    wakers: Mutex<Vec<rune::Waker>>,
    /// The Rune window of each store Window that has one.
    open: Mutex<HashMap<WindowId, WindowHandle>>,
}

/// Runs the windowed app until its last Window is closed.
pub fn run_app(store: Arc<AppStore>, events: StoreEvents) {
    let shared = Arc::new(Shared {
        store,
        wakers: Mutex::new(Vec::new()),
        open: Mutex::new(HashMap::new()),
    });
    let follower = Arc::clone(&shared);
    let spawned = std::thread::Builder::new()
        .name("tafit-ui-events".to_owned())
        .spawn(move || {
            while events.recv_blocking().is_ok() {
                for wake in lock(&follower.wakers).iter() {
                    wake();
                }
            }
        });
    if let Err(err) = spawned {
        eprintln!("{APP_NAME}: could not follow the store: {err}");
        return;
    }
    let first = shared
        .store
        .snapshot()
        .windows
        .first()
        .map_or(WindowId::FIRST, |window| window.id);
    rune::run_windows(move |_| vec![window_spec(&shared, first)]);
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn window_spec(shared: &Arc<Shared>, id: WindowId) -> WindowSpec<Ui> {
    let spec = AppSpec {
        fonts: crate::fonts::BUNDLED,
        families: crate::fonts::FAMILIES,
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(crate::diff::on_key),
        ..AppSpec::new(APP_NAME, (1100.0, 720.0), no_init, crate::view)
    };
    let state = Arc::clone(shared);
    let app = App::new(spec)
        .init(move || Ui::new(state, id))
        .modifier_keys()
        .on_close(on_close);
    let ready = Arc::clone(shared);
    WindowSpec::from_app(window_key(id), app)
        .on_focus(on_focus)
        .on_modifiers(on_modifiers)
        .on_ready(move |handle| {
            lock(&ready.open).insert(id, handle.clone());
        })
}

/// `AppSpec` needs a plain `fn`; every window gets its state from `App::init`.
fn no_init() -> Ui {
    unreachable!("each window's state comes from App::init")
}

fn window_key(id: WindowId) -> String {
    format!("window-{id:?}")
}

fn setup(s: &mut Ui, waker: rune::Waker) {
    lock(&s.shared.wakers).push(waker);
}

/// At the start of each frame: the latest snapshot, then the windows it
/// asks for. A diff loading for long enough redraws too, to say so.
fn poll(s: &mut Ui, _: f64, tx: &mut Tx) -> bool {
    let changed = s.refresh();
    let waited = s.diff_wait_elapsed(tx);
    if std::mem::take(&mut s.diff.switched) {
        tx.instant();
    }
    if changed {
        sync_windows(s, tx);
    }
    changed || waited
}

/// Opens the store's new Windows, closes the ones it dropped, and brings
/// this one to the front when asked.
fn sync_windows(s: &mut Ui, tx: &mut Tx) {
    let state = &s.state;
    if state.windows.is_empty() {
        // Nothing for the user to see or close.
        tx.quit();
        return;
    }
    let mut open = lock(&s.shared.open);
    open.retain(|id, handle| {
        let keep = state.window(*id).is_some();
        if !keep {
            handle.close();
        }
        keep
    });
    for window in &state.windows {
        if window.id != s.window && !open.contains_key(&window.id) {
            let handle = rune::open_window(window_spec(&s.shared, window.id));
            open.insert(window.id, handle);
        }
    }
    if let Some(window) = state.window(s.window)
        && window.raise_requests > s.raised
    {
        s.raised = window.raise_requests;
        if let Some(handle) = open.get(&s.window) {
            handle.focus();
        }
    }
}

fn on_close(s: &mut Ui, _: &mut Tx) {
    lock(&s.shared.open).remove(&s.window);
    s.dispatch(Msg::CloseWindow(s.window));
}

fn on_focus(s: &mut Ui, focused: bool, _: &mut Tx) {
    if focused {
        s.dispatch(Msg::FocusWindow(s.window));
    }
}

fn on_modifiers(s: &mut Ui, mods: rune::Mods, _: &mut Tx) {
    s.mods = mods;
}
