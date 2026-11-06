//! Keeps GPUI's windows in step with the store's: one GPUI window per
//! `WindowState`, each showing it. One poller for the whole app re-reads the
//! snapshot after every store change and hands it to every Window.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Bounds, WindowBounds, WindowHandle, WindowOptions, prelude::*, px, size};
use tafit_state::{AppStore, Msg, StoreEvents, WindowId};

use crate::{TafitView, window_icon};

pub(crate) struct Host {
    store: Arc<AppStore>,
    windows: HashMap<WindowId, OpenWindow>,
}

struct OpenWindow {
    handle: WindowHandle<TafitView>,
    /// The `raise_requests` already acted on.
    raised: u64,
}

impl Host {
    /// Opens the store's Windows, then follows the store until it stops.
    pub(crate) fn start(store: Arc<AppStore>, events: StoreEvents, cx: &mut App) {
        let mut host = Self {
            store,
            windows: HashMap::new(),
        };
        host.sync(cx);
        cx.spawn(async move |cx| {
            while events.recv().await.is_ok() {
                cx.update(|cx| host.sync(cx));
            }
        })
        .detach();
    }

    fn sync(&mut self, cx: &mut App) {
        let state = self.store.snapshot();
        // Closed in the store (or by the user, whose close already told it).
        self.windows.retain(|id, open| {
            let keep = state.window(*id).is_some();
            if !keep {
                let _ = open
                    .handle
                    .update(cx, |_, window, _| window.remove_window());
            }
            keep
        });
        for window in &state.windows {
            if !self.windows.contains_key(&window.id) {
                match self.open(window.id, cx) {
                    Some(handle) => {
                        self.windows.insert(
                            window.id,
                            OpenWindow {
                                handle,
                                raised: window.raise_requests,
                            },
                        );
                    }
                    // Can't show it; drop it from the store so it isn't kept.
                    None => self.store.dispatch(Msg::CloseWindow(window.id)),
                }
                continue;
            }
            let open = self.windows.get_mut(&window.id).expect("checked above");
            let raise = window.raise_requests > open.raised;
            open.raised = window.raise_requests;
            let state = Arc::clone(&state);
            let _ = open.handle.update(cx, |view, gpui_window, cx| {
                view.set_state(state, cx);
                if raise {
                    gpui_window.activate();
                }
            });
        }
        // No Window left, or none could be opened: nothing for the user to
        // see or close, so stop rather than run on without a UI.
        if self.windows.is_empty() {
            cx.quit();
        }
    }

    fn open(&self, id: WindowId, cx: &mut App) -> Option<WindowHandle<TafitView>> {
        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(tafit_base::APP_NAME.into()),
                ..Default::default()
            }),
            icon: window_icon(),
            ..Default::default()
        };
        let store = Arc::clone(&self.store);
        let opened = cx.open_window(options, |window, cx| {
            let closing = Arc::clone(&store);
            window.on_window_should_close(cx, move |_, _| {
                closing.dispatch(Msg::CloseWindow(id));
                true
            });
            cx.new(|cx| {
                cx.observe_window_activation(window, |view: &mut TafitView, window, _| {
                    if window.is_window_active() {
                        view.store.dispatch(Msg::FocusWindow(view.window));
                    }
                })
                .detach();
                TafitView::new(store, id, cx)
            })
        });
        match opened {
            Ok(handle) => Some(handle),
            Err(err) => {
                eprintln!("{}: could not open a window: {err:#}", tafit_base::APP_NAME);
                None
            }
        }
    }
}
