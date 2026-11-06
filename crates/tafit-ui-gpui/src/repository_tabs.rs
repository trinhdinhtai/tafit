//! The tab bar's Repository Tabs: click to select, middle-click or × to
//! close, drag onto another tab to move there. Scrolls sideways when the
//! tabs don't fit.

use gpui::{Context, IntoElement, SharedString, Stateful, div, prelude::*, px};
use tafit_state::{AppState, Loadable, Msg, RepoId, RepoState, WindowId};
use tafit_ui_kit::{Theme, button, close_button, tab};

use crate::TafitView;

/// What a dragged Repository Tab carries.
#[derive(Clone)]
struct DraggedTab {
    repo_id: RepoId,
    /// Where the tab was when the drag started.
    ix: usize,
    label: SharedString,
}

/// Follows the pointer while a tab is dragged.
struct DraggedTabView {
    label: SharedString,
    theme: Theme,
}

impl Render for DraggedTabView {
    fn render(&mut self, _window: &mut gpui::Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .h(px(34.))
            .bg(self.theme.surface)
            .border_1()
            .border_color(self.theme.accent)
            .opacity(0.9)
            .child(tab(self.label.clone(), true, &self.theme))
    }
}

/// `window`'s Repository Tabs, in a strip that scrolls sideways.
pub(crate) fn repository_tabs(
    state: &AppState,
    window: WindowId,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> impl IntoElement + use<> {
    let active = state.window(window).and_then(|window| window.active_repo);
    let tabs: Vec<_> = state
        .tabs(window)
        .enumerate()
        .map(|(ix, repo)| repository_tab(repo, ix, Some(repo.id) == active, theme, cx))
        .collect();
    div()
        .id("repository-tabs")
        .flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .overflow_x_scroll()
        .children(tabs)
}

fn repository_tab(
    repo: &RepoState,
    ix: usize,
    active: bool,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> Stateful<gpui::Div> {
    let repo_id = repo.id;
    let label = tab_label(repo);
    let dragged = DraggedTab {
        repo_id,
        ix,
        label: label.clone(),
    };
    let accent = theme.accent;
    let drag_theme = *theme;
    div()
        .id(("repository-tab", repo_id.as_u64()))
        .debug_selector(move || format!("repository-tab-{ix}"))
        .flex_none()
        .h_full()
        .child(
            tab(label, active, theme).child(
                close_button(theme)
                    .id(("close-tab", repo_id.as_u64()))
                    .debug_selector(|| "close-tab".to_owned())
                    .on_click(cx.listener(move |view, _, _, cx| {
                        cx.stop_propagation();
                        view.store.dispatch(Msg::CloseRepo(repo_id));
                    })),
            ),
        )
        .on_click(cx.listener(move |view, _, _, _| {
            view.store.dispatch(Msg::ActivateRepo(repo_id));
        }))
        .on_aux_click(cx.listener(move |view, event: &gpui::ClickEvent, _, _| {
            if event.is_middle_click() {
                view.store.dispatch(Msg::CloseRepo(repo_id));
            }
        }))
        .on_drag(dragged, move |dragged, _, _, cx| {
            cx.new(|_| DraggedTabView {
                label: dragged.label.clone(),
                theme: drag_theme,
            })
        })
        // The dragged tab takes this one's place: dragged from the left it
        // lands after it, from the right before it. The marker shows which.
        .on_drag_over::<DraggedTab>(move |style, dragged, _, _| {
            let style = if dragged.ix < ix {
                style.border_r_2()
            } else {
                style.border_l_2()
            };
            style.border_color(accent)
        })
        .on_drop(cx.listener(move |view, dragged: &DraggedTab, _, _| {
            view.store.dispatch(Msg::MoveTab {
                repo_id: dragged.repo_id,
                to: ix,
            });
        }))
}

/// A compact tab bar button; it clears the 34 px bar by ~5 px above and below.
pub(crate) fn tab_bar_button(
    label: &'static str,
    selector: &'static str,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    button(label, theme)
        .py(px(2.))
        .text_xs()
        .id(selector)
        .debug_selector(move || selector.to_owned())
}

pub(crate) fn tab_label(repo: &RepoState) -> SharedString {
    match &repo.open {
        Loadable::Ready(info) => info.name.clone().into(),
        // Not opened (yet): fall back to the folder name.
        _ => folder_name(&repo.path).into(),
    }
}

/// The last part of `path`, or all of it for a root such as `/`.
pub(crate) fn folder_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
