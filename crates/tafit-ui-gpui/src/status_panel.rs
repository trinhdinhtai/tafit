//! The change list of the Active Repository: staged, unstaged and untracked
//! files in one virtualized list, so only the rows on screen are built.

use std::sync::Arc;

use gpui::{
    AnyElement, Context, Hsla, IntoElement, SharedString, div, prelude::*, px, uniform_list,
};
use tafit_core::{ChangeKind, FileChange, RepositoryStatus};
use tafit_state::{Loadable, Msg, RepoState};
use tafit_ui_kit::{Theme, button, fonts};

use crate::{TafitView, centered_message};

const ROW_HEIGHT: f32 = 24.;

pub(crate) fn status_panel(
    repo: &RepoState,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> impl IntoElement {
    let repo_id = repo.id;
    let refresh_label = if repo.status_refreshing {
        "Refreshing…"
    } else {
        "Refresh"
    };
    let header = div()
        .flex()
        .flex_none()
        .items_center()
        .px(px(14.))
        .py(px(6.))
        .border_b_1()
        .border_color(theme.border)
        .child(div().flex_1().text_color(theme.text_muted).child("Changes"))
        .child(
            button(refresh_label, theme)
                .id("status-refresh")
                .debug_selector(|| "status-refresh".to_owned())
                .on_click(cx.listener(move |view, _, _, _| {
                    view.store.dispatch(Msg::RefreshStatus(repo_id));
                })),
        );
    let body = match &repo.status {
        Loadable::Loading => {
            centered_message("Reading changes…", theme.text_muted).into_any_element()
        }
        Loadable::Error(message) => {
            centered_message(message.clone(), theme.error).into_any_element()
        }
        Loadable::Ready(status) if status.is_clean() => div()
            .debug_selector(|| "status-clean".to_owned())
            .size_full()
            .child(centered_message("No changes", theme.text_muted))
            .into_any_element(),
        Loadable::Ready(status) => change_list(Arc::clone(status), *theme).into_any_element(),
    };
    div()
        .debug_selector(|| "status-panel".to_owned())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .border_t_1()
        .border_color(theme.border)
        .child(header)
        .child(div().flex_1().min_h_0().child(body))
}

/// The three groups flattened into rows: each non-empty group is a header row
/// followed by its files.
fn change_list(status: Arc<RepositoryStatus>, theme: Theme) -> impl IntoElement {
    let row_count = groups(&status)
        .iter()
        .filter(|group| !group.files.is_empty())
        .map(|group| group.files.len() + 1)
        .sum();
    uniform_list("status-list", row_count, move |range, _window, _cx| {
        range
            .map(|ix| match row(&status, ix) {
                Row::Header(group) => group_header(group, &theme),
                Row::File(file) => file_row(file, &theme),
            })
            .collect::<Vec<_>>()
    })
    .size_full()
}

struct Group<'a> {
    title: &'static str,
    selector: &'static str,
    files: &'a [FileChange],
}

fn groups(status: &RepositoryStatus) -> [Group<'_>; 3] {
    [
        Group {
            title: "Staged",
            selector: "status-staged",
            files: &status.staged,
        },
        Group {
            title: "Unstaged",
            selector: "status-unstaged",
            files: &status.unstaged,
        },
        Group {
            title: "Untracked",
            selector: "status-untracked",
            files: &status.untracked,
        },
    ]
}

enum Row<'a> {
    Header(Group<'a>),
    File(&'a FileChange),
}

/// Row `ix` of the flattened list; constant time, so scrolling costs the same
/// at row ten and row ten thousand.
fn row(status: &RepositoryStatus, mut ix: usize) -> Row<'_> {
    for group in groups(status) {
        if group.files.is_empty() {
            continue;
        }
        if ix == 0 {
            return Row::Header(group);
        }
        if let Some(file) = group.files.get(ix - 1) {
            return Row::File(file);
        }
        ix -= group.files.len() + 1;
    }
    unreachable!("uniform_list asks only for rows below its row count")
}

fn group_header(group: Group<'_>, theme: &Theme) -> AnyElement {
    let selector = group.selector;
    div()
        .debug_selector(move || selector.to_owned())
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .px(px(14.))
        .bg(theme.surface)
        .text_color(theme.text_muted)
        .child(format!("{} ({})", group.title, group.files.len()))
        .into_any_element()
}

fn file_row(file: &FileChange, theme: &Theme) -> AnyElement {
    let (letter, color, label): (&str, Hsla, SharedString) = match &file.kind {
        ChangeKind::Added => ("A", theme.success, file.path.clone().into()),
        ChangeKind::Modified => ("M", theme.warning, file.path.clone().into()),
        ChangeKind::Deleted => ("D", theme.error, file.path.clone().into()),
        ChangeKind::Renamed { from } => {
            ("R", theme.accent, format!("{from} → {}", file.path).into())
        }
    };
    div()
        .debug_selector(|| "status-row".to_owned())
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(14.))
        .child(
            div()
                .w(px(12.))
                .flex_none()
                .font_family(fonts::MONOSPACE_FONT_FAMILY)
                .text_color(color)
                .child(letter),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
        .into_any_element()
}
