//! The change list of the Active Repository: staged, unstaged and untracked
//! files in one virtualized list, so only the rows on screen are built. Below
//! it, the commit message box.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::{
    AnyElement, ColorExt, Context, Entity, Hsla, IntoElement, SharedString, div, prelude::*, px,
    uniform_list,
};
use tafit_core::{ChangeKind, FileChange, RepositoryStatus};
use tafit_state::{AppStore, ChangeGroup, FileRef, Loadable, Msg, RepoId, RepoState, SelectMode};
use tafit_ui_kit::text::{TextState, text_area};
use tafit_ui_kit::{Theme, button, disabled_button, fonts};

use crate::{CommitStaged, TafitView, centered_message};

const ROW_HEIGHT: f32 = 24.;

/// The key context of the commit box, where Ctrl/Cmd-Enter commits.
pub(crate) const COMMIT_BOX_CONTEXT: &str = "CommitBox";

pub(crate) fn status_panel(
    repo: &RepoState,
    store: &Arc<AppStore>,
    commit_message: &Entity<TextState>,
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
        .gap(px(6.))
        .px(px(14.))
        .py(px(6.))
        .border_b_1()
        .border_color(theme.border)
        .child(div().flex_1().text_color(theme.text_muted).child("Changes"))
        .child(write_button(repo, true, theme, cx))
        .child(write_button(repo, false, theme, cx))
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
        Loadable::Ready(status) => change_list(
            Arc::clone(status),
            ListContext {
                repo_id,
                selection: Arc::new(repo.selection.clone()),
                store: Arc::clone(store),
                theme: *theme,
            },
        )
        .into_any_element(),
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
        .child(commit_box(repo, commit_message, theme, cx))
}

/// The Stage (or Unstage) button for the selection; muted when the selection
/// has nothing to stage (unstage) or a write is running.
fn write_button(
    repo: &RepoState,
    stage: bool,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> AnyElement {
    let (label, selector) = if stage {
        ("Stage", "stage-selected")
    } else {
        ("Unstage", "unstage-selected")
    };
    let applies = repo
        .selection
        .iter()
        .any(|file| (file.group == ChangeGroup::Staged) != stage);
    if repo.writing || !applies {
        return disabled_button(label, theme)
            .debug_selector(move || format!("{selector}-disabled"))
            .into_any_element();
    }
    let repo_id = repo.id;
    button(label, theme)
        .id(selector)
        .debug_selector(move || selector.to_owned())
        .on_click(cx.listener(move |view, _, _, _| {
            view.store.dispatch(if stage {
                Msg::StageSelected(repo_id)
            } else {
                Msg::UnstageSelected(repo_id)
            });
        }))
        .into_any_element()
}

/// The message box, the last write's error, and the commit button.
fn commit_box(
    repo: &RepoState,
    commit_message: &Entity<TextState>,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> impl IntoElement {
    let repo_id = repo.id;
    let refusal = repo.commit_refusal();
    let commit_button = if repo.writing {
        disabled_button("Committing…", theme).into_any_element()
    } else if refusal.is_some() {
        disabled_button("Commit", theme)
            .debug_selector(|| "commit-button-disabled".to_owned())
            .into_any_element()
    } else {
        button("Commit", theme)
            .id("commit-button")
            .debug_selector(|| "commit-button".to_owned())
            .on_click(cx.listener(move |view, _, _, _| {
                view.store.dispatch(Msg::Commit(repo_id));
            }))
            .into_any_element()
    };
    let hint = refusal
        .filter(|_| !repo.writing)
        .map(|refusal| div().text_color(theme.text_muted).text_xs().child(refusal));
    // Shown whole: a hook's output can run long, so it scrolls.
    let error = repo.write_error.clone().map(|error| {
        div()
            .id("write-error")
            .debug_selector(|| "write-error".to_owned())
            .max_h(px(160.))
            .overflow_y_scroll()
            .font_family(fonts::MONOSPACE_FONT_FAMILY)
            .text_xs()
            .text_color(theme.error)
            .child(error)
    });
    div()
        .key_context(COMMIT_BOX_CONTEXT)
        .on_action(cx.listener(move |view, _: &CommitStaged, _, _| {
            view.store.dispatch(Msg::Commit(repo_id));
        }))
        .flex()
        .flex_col()
        .flex_none()
        .gap(px(6.))
        .p(px(10.))
        .border_t_1()
        .border_color(theme.border)
        .child(
            div().debug_selector(|| "commit-message".to_owned()).child(
                text_area("commit-message", commit_message, "Commit message", theme)
                    // The store freezes the message while it is committed.
                    .accepts_input(!repo.committing)
                    .w_full()
                    .h(px(84.)),
            ),
        )
        .children(error)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().flex_1().min_w_0().children(hint))
                .child(commit_button),
        )
}

/// What every row of the change list needs besides its file.
#[derive(Clone)]
struct ListContext {
    repo_id: RepoId,
    selection: Arc<BTreeSet<FileRef>>,
    store: Arc<AppStore>,
    theme: Theme,
}

/// The three groups flattened into rows: each non-empty group is a header row
/// followed by its files.
fn change_list(status: Arc<RepositoryStatus>, list: ListContext) -> impl IntoElement {
    let row_count = groups(&status)
        .iter()
        .filter(|group| !group.files.is_empty())
        .map(|group| group.files.len() + 1)
        .sum();
    uniform_list("status-list", row_count, move |range, _window, _cx| {
        range
            .map(|ix| match row(&status, ix) {
                Row::Header(group) => group_header(group, &list.theme),
                Row::File(group, file) => file_row(ix, group, file, &list),
            })
            .collect::<Vec<_>>()
    })
    .size_full()
}

struct Group<'a> {
    group: ChangeGroup,
    title: &'static str,
    selector: &'static str,
    files: &'a [FileChange],
}

fn groups(status: &RepositoryStatus) -> [Group<'_>; 3] {
    [
        (ChangeGroup::Staged, "Staged", "status-staged"),
        (ChangeGroup::Unstaged, "Unstaged", "status-unstaged"),
        (ChangeGroup::Untracked, "Untracked", "status-untracked"),
    ]
    .map(|(group, title, selector)| Group {
        group,
        title,
        selector,
        files: group.files(status),
    })
}

enum Row<'a> {
    Header(Group<'a>),
    File(ChangeGroup, &'a FileChange),
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
            return Row::File(group.group, file);
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

/// A click selects the file, Ctrl/Cmd-click adds or removes it, and a
/// double-click stages it (or unstages a staged one).
fn file_row(ix: usize, group: ChangeGroup, file: &FileChange, list: &ListContext) -> AnyElement {
    let theme = &list.theme;
    let (letter, color, label): (&str, Hsla, SharedString) = match &file.kind {
        ChangeKind::Added => ("A", theme.success, file.path.clone().into()),
        ChangeKind::Modified => ("M", theme.warning, file.path.clone().into()),
        ChangeKind::Deleted => ("D", theme.error, file.path.clone().into()),
        ChangeKind::Renamed { from } => {
            ("R", theme.accent, format!("{from} → {}", file.path).into())
        }
    };
    let target = FileRef {
        group,
        path: file.path.clone(),
    };
    let selected = list.selection.contains(&target);
    let (store, repo_id) = (Arc::clone(&list.store), list.repo_id);
    let hover = theme.surface;
    div()
        .id(("status-row", ix))
        .debug_selector(|| "status-row".to_owned())
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(14.))
        .cursor_pointer()
        .when(selected, |row| row.bg(theme.accent.opacity(0.3)))
        .when(!selected, |row| {
            row.hover_style(move |style| style.bg(hover))
        })
        .on_click(move |event, _, _| {
            let mode = if event.modifiers().secondary() {
                SelectMode::Toggle
            } else {
                SelectMode::Only
            };
            store.dispatch(Msg::Select {
                repo_id,
                file: target.clone(),
                mode,
            });
            if mode == SelectMode::Only && event.click_count() == 2 {
                store.dispatch(if group == ChangeGroup::Staged {
                    Msg::UnstageSelected(repo_id)
                } else {
                    Msg::StageSelected(repo_id)
                });
            }
        })
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
