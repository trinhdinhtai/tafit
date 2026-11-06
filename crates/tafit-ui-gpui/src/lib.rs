//! The GPUI host: opens the Window and draws `AppState` snapshots. Holds no
//! app state of its own; every change goes through the store.

use std::sync::Arc;

use gpui::{
    App, Bounds, Context, IntoElement, Render, SharedString, Task, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use tafit_core::Head;
use tafit_state::{AppState, AppStore, Loadable, RepoState, StoreEvents};
use tafit_ui_kit::{Theme, tab, tab_bar};

/// Runs the windowed app until the Window is closed.
pub fn run(store: Arc<AppStore>, events: StoreEvents) {
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some(tafit_base::APP_NAME.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| TafitView::new(store, Some(events), cx)),
        )
        .expect("open the main window");
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}

#[cfg(target_os = "windows")]
fn application() -> gpui::Application {
    // wgpu (DX12) where available, falling back to DX11.
    gpui_platform::application_with_windows_renderer(gpui_platform::WindowsRendererOptions {
        preference: gpui_platform::WindowsRendererPreference::Auto,
        on_event: None,
    })
}

#[cfg(not(target_os = "windows"))]
fn application() -> gpui::Application {
    gpui_platform::application()
}

/// The Window's root view: a tab bar of Repository Tabs over the active one.
pub struct TafitView {
    _store: Arc<AppStore>,
    state: Arc<AppState>,
    theme: Theme,
    _poller: Option<Task<()>>,
}

impl TafitView {
    /// With `events`, a poller re-reads the snapshot after every store change.
    /// Pass `None` under GPUI's test scheduler, which can't be woken from
    /// another thread.
    pub fn new(store: Arc<AppStore>, events: Option<StoreEvents>, cx: &mut Context<Self>) -> Self {
        let poller = events.map(|events| {
            let store = Arc::clone(&store);
            cx.spawn(async move |view, cx| {
                while events.recv().await.is_ok() {
                    let snapshot = store.snapshot();
                    let updated = view.update(cx, |view, cx| {
                        view.state = snapshot;
                        cx.notify();
                    });
                    if updated.is_err() {
                        break; // The view is gone.
                    }
                }
            })
        });
        Self {
            state: store.snapshot(),
            _store: store,
            theme: Theme::dark(),
            _poller: poller,
        }
    }
}

impl Render for TafitView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let active = self.state.active_repo;
        let tabs = self.state.repos.iter().map(|repo| {
            div()
                .debug_selector(|| "repository-tab".to_owned())
                .h_full()
                .child(tab(tab_label(repo), Some(repo.id) == active, theme))
        });
        let body = match self.state.active() {
            Some(repo) => repository_tab_body(repo, theme).into_any_element(),
            None => centered_message("No Repository open", theme.text_muted).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.text)
            .text_sm()
            .child(tab_bar(theme).children(tabs))
            .child(body)
    }
}

fn tab_label(repo: &RepoState) -> SharedString {
    match &repo.open {
        Loadable::Ready(info) => info.name.clone().into(),
        // Not opened (yet): fall back to the folder name.
        _ => repo
            .path
            .file_name()
            .unwrap_or(repo.path.as_os_str())
            .to_string_lossy()
            .into_owned()
            .into(),
    }
}

fn repository_tab_body(repo: &RepoState, theme: &Theme) -> impl IntoElement {
    match &repo.open {
        Loadable::Loading => centered_message("Opening…", theme.text_muted).into_any_element(),
        Loadable::Error(message) => div()
            .debug_selector(|| "repository-error".to_owned())
            .child(centered_message(message.clone(), theme.error))
            .size_full()
            .into_any_element(),
        Loadable::Ready(info) => div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .p(px(24.))
            .child(
                div()
                    .debug_selector(|| "repository-name".to_owned())
                    .text_xl()
                    .child(info.name.clone()),
            )
            .child(
                div()
                    .debug_selector(|| "repository-head".to_owned())
                    .text_color(theme.text_muted)
                    .child(head_label(&info.head)),
            )
            .into_any_element(),
    }
}

fn head_label(head: &Head) -> String {
    match head {
        Head::Branch(branch) => format!("On branch {branch}"),
        Head::Unborn(branch) => format!("On branch {branch} (no commits yet)"),
        Head::Detached(commit) => format!("HEAD detached at {commit}"),
    }
}

fn centered_message(text: impl Into<SharedString>, color: gpui::Hsla) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(color)
        .child(text.into())
}
