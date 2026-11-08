//! The tab bar: the Window's Repository Tabs in `rune-kit`'s tab strip
//! (click to select, middle-click or × to close, drag to move), then the
//! buttons that open a Repository, a Window or Settings.

use std::path::Path;

use rune::{Cx, El, Tx, row};
use rune_kit::shell::{TabItem, TabOrientation, TabReorder, TabStrip};
use rune_kit::{Button, ButtonVariant, Kit};
use tafit_state::{Loadable, Msg, Page, RepoId, RepoState};

use crate::Ui;

pub(crate) fn tab_bar(s: &Ui, cx: &mut Cx<Ui>, k: &Kit, layer: &mut Vec<El>) -> El {
    let active = s
        .state
        .window(s.window)
        .and_then(|window| window.active_repo)
        .map(tab_id);
    let items: Vec<TabItem<'_, Ui>> = s
        .state
        .tabs(s.window)
        .map(|repo| {
            TabItem::new(tab_id(repo.id), tab_label(repo))
                .icon(&rune_kit::glyphs::FOLDER)
                .closable(true)
        })
        .collect();
    let strip = TabStrip::new("repository-tabs", TabOrientation::Horizontal)
        .label("Repository Tabs")
        .tabs(items)
        .active(active.as_deref())
        .viewport_width(cx.layout_size("repository-tabs-area").map(|size| size[0]))
        .drag_state(&s.tab_drag, |s: &mut Ui| &mut s.tab_drag)
        .on_select(|s: &mut Ui, id: &str, _: &mut Tx| {
            if let Some(repo_id) = s.tab_repo(id) {
                s.dispatch(Msg::ActivateRepo(repo_id));
            }
        })
        .on_close(|s: &mut Ui, id: &str, _: &mut Tx| {
            if let Some(repo_id) = s.tab_repo(id) {
                s.dispatch(Msg::CloseRepo(repo_id));
            }
        })
        .on_reorder(|s: &mut Ui, moved: TabReorder, _: &mut Tx| {
            if let Some(repo_id) = s.tab_repo(&moved.id) {
                s.dispatch(Msg::MoveTab {
                    repo_id,
                    to: moved.to,
                });
            }
        })
        .build_layered(k, layer);
    let button = |label: &str, key: &str, page: Option<Page>| {
        Button::new(label)
            .variant(ButtonVariant::Ghost)
            .on_press(move |s: &mut Ui| match page {
                Some(page) => s.show_page(page),
                None => s.dispatch(Msg::OpenWindow),
            })
            .build(k)
            .key(key)
    };
    row()
        .key("tab-bar")
        .w_full()
        .h(40.0)
        .px(8.0)
        .gap(6.0)
        .items_center()
        .fill(k.c.layer_01)
        .child(
            row()
                .key("repository-tabs-area")
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .items_center()
                .child(strip),
        )
        .child(button(
            "+",
            "open-repository-button",
            Some(Page::OpenRepository),
        ))
        .child(button("New Window", "new-window-button", None))
        .child(button("Settings", "settings-button", Some(Page::Settings)))
}

impl Ui {
    /// The Repository of this Window's tab `id`.
    fn tab_repo(&self, id: &str) -> Option<RepoId> {
        self.state
            .tabs(self.window)
            .map(|repo| repo.id)
            .find(|&repo_id| tab_id(repo_id) == id)
    }
}

fn tab_id(repo_id: RepoId) -> String {
    repo_id.as_u64().to_string()
}

fn tab_label(repo: &RepoState) -> String {
    match &repo.open {
        Loadable::Ready(info) => info.name.clone(),
        // Not opened (yet): fall back to the folder name.
        _ => folder_name(&repo.path),
    }
}

/// The last part of `path`, or all of it for a root such as `/`.
pub(crate) fn folder_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}
