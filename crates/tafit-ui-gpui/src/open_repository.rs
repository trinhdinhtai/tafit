//! The Open Repository page: a folder picker, and the pinned and recent
//! Repositories to open again with one click. Shown in a Window with no
//! Repository Tab, or from the tab bar's "+".

use std::path::{Path, PathBuf};

use gpui::{Context, IntoElement, PathPromptOptions, div, prelude::*, px};
use tafit_state::{AppState, Msg, Page, WindowId};
use tafit_ui_kit::{Theme, button};

use crate::TafitView;
use crate::repository_tabs::folder_name;

pub(crate) fn open_repository_page(
    state: &AppState,
    window: WindowId,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> impl IntoElement + use<> {
    let has_tabs = state.tabs(window).next().is_some();
    let close = has_tabs.then(|| {
        button("Close", theme)
            .id("close-open-repository")
            .on_click(cx.listener(|view, _, _, _| view.show_page(Page::RepositoryTab)))
    });
    let rows: Vec<_> = state
        .repository_list()
        .enumerate()
        .map(|(ix, (path, pinned))| repository_row(ix, path, pinned, window, theme, cx))
        .collect();
    let list = if rows.is_empty() {
        div()
            .text_color(theme.text_muted)
            .child("Repositories you open are listed here.")
            .into_any_element()
    } else {
        div()
            .id("repository-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .children(rows)
            .into_any_element()
    };
    div()
        .debug_selector(|| "open-repository-page".to_owned())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .gap(px(12.))
        .p(px(24.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().text_xl().child("Open a Repository"))
                .child(
                    button("Open Folder…", theme)
                        .id("open-folder")
                        .debug_selector(|| "open-folder".to_owned())
                        .on_click(cx.listener(|view, _, _, cx| view.prompt_for_repository(cx))),
                )
                .children(close),
        )
        .child(
            div()
                .text_color(theme.text_muted)
                .child("Pinned and recent"),
        )
        .child(list)
}

fn repository_row(
    ix: usize,
    path: &Path,
    pinned: bool,
    window: WindowId,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> impl IntoElement + use<> {
    let name = folder_name(path);
    let open_path = path.to_owned();
    let pin_path: PathBuf = path.to_owned();
    let (pin_label, pin_selector) = if pinned {
        ("Unpin", "unpin-repository")
    } else {
        ("Pin", "pin-repository")
    };
    let hover = theme.surface;
    div()
        .id(("repository-list-row", ix))
        .debug_selector(|| "repository-list-row".to_owned())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(12.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover_style(move |style| style.bg(hover))
        .on_click(cx.listener(move |view, _, _, _| {
            view.store.dispatch(Msg::OpenRepo {
                window,
                path: open_path.clone(),
            });
        }))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(name)
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(path.display().to_string()),
                ),
        )
        .child(
            button(pin_label, theme)
                .text_xs()
                .id(("pin-repository", ix))
                .debug_selector(move || pin_selector.to_owned())
                .on_click(cx.listener(move |view, _, _, cx| {
                    cx.stop_propagation();
                    view.store.dispatch(if pinned {
                        Msg::UnpinRepo(pin_path.clone())
                    } else {
                        Msg::PinRepo(pin_path.clone())
                    });
                })),
        )
}

impl TafitView {
    /// Asks for a folder and opens it in this Window.
    pub(crate) fn prompt_for_repository(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open Repository".into()),
            directory: None,
        });
        let store = std::sync::Arc::clone(&self.store);
        let window = self.window;
        cx.spawn(async move |_, _| {
            // Cancelled, or the picker couldn't open: nothing to do.
            if let Ok(Ok(Some(paths))) = picked.await {
                for path in paths {
                    store.dispatch(Msg::OpenRepo { window, path });
                }
            }
        })
        .detach();
    }
}
