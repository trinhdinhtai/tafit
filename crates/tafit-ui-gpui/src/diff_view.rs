//! The inline Line Diff of the selected file: every hunk's header and lines
//! in one virtualized list, so a diff of a huge file builds only the rows on
//! screen.

use std::sync::Arc;

use gpui::{AnyElement, ColorExt, Hsla, IntoElement, div, prelude::*, px, uniform_list};
use tafit_core::{DiffLine, FileDiff, Hunk, LineKind};
use tafit_state::{Loadable, RepoState};
use tafit_ui_kit::{Theme, fonts};

use crate::centered_message;

const ROW_HEIGHT: f32 = 20.;

/// Characters of a line drawn; shaping a minified file's one huge line would
/// stall every frame it is on screen.
// shortcut: the rest of a longer line is cut off until the diff scrolls
// sideways or wraps (#10).
const MAX_LINE_CHARS: usize = 1_000;

pub(crate) fn diff_view(repo: &RepoState, theme: &Theme) -> impl IntoElement {
    let body = match &repo.diff {
        None => {
            let hint = if repo.selection.len() > 1 {
                format!("{} files selected", repo.selection.len())
            } else {
                "Select a file to see its changes".to_owned()
            };
            div()
                .debug_selector(|| "diff-empty".to_owned())
                .size_full()
                .child(centered_message(hint, theme.text_muted))
                .into_any_element()
        }
        Some(diff) => match &diff.diff {
            Loadable::Loading => {
                centered_message("Reading diff…", theme.text_muted).into_any_element()
            }
            Loadable::Error(message) => {
                centered_message(message.clone(), theme.error).into_any_element()
            }
            Loadable::Ready(loaded) => match &**loaded {
                FileDiff::Binary => div()
                    .debug_selector(|| "diff-binary".to_owned())
                    .size_full()
                    .child(centered_message(
                        "Binary file: no line diff to show",
                        theme.text_muted,
                    ))
                    .into_any_element(),
                FileDiff::Text(hunks) if hunks.is_empty() => {
                    centered_message("No content changes", theme.text_muted).into_any_element()
                }
                FileDiff::Text(_) => lines(Arc::clone(loaded), *theme).into_any_element(),
                // Nothing picks the two-column layout yet.
                FileDiff::TwoColumn(_) => {
                    centered_message("The two-column layout isn't drawn yet", theme.text_muted)
                        .into_any_element()
                }
            },
        },
    };
    div()
        .debug_selector(|| "diff-view".to_owned())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .border_l_1()
        .border_t_1()
        .border_color(theme.border)
        .font_family(fonts::MONOSPACE_FONT_FAMILY)
        .text_xs()
        .child(body)
}

fn lines(diff: Arc<FileDiff>, theme: Theme) -> impl IntoElement {
    let FileDiff::Text(hunks) = &*diff else {
        unreachable!("only text diffs have lines")
    };
    // The row each hunk's header is on; a hunk is its header and its lines.
    let mut starts = Vec::with_capacity(hunks.len());
    let mut row_count = 0;
    for hunk in hunks {
        starts.push(row_count);
        row_count += hunk.lines.len() + 1;
    }
    uniform_list("diff-lines", row_count, move |range, _window, _cx| {
        let FileDiff::Text(hunks) = &*diff else {
            unreachable!("checked above")
        };
        range
            .map(|ix| {
                // The last hunk starting at or before `ix`.
                let hunk = starts.partition_point(|&start| start <= ix) - 1;
                match ix - starts[hunk] {
                    0 => hunk_header(&hunks[hunk], &theme),
                    line => diff_line(&hunks[hunk].lines[line - 1], &theme),
                }
            })
            .collect::<Vec<_>>()
    })
    .size_full()
}

fn hunk_header(hunk: &Hunk, theme: &Theme) -> AnyElement {
    div()
        .debug_selector(|| "diff-hunk-header".to_owned())
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .px(px(8.))
        .bg(theme.surface)
        .text_color(theme.text_muted)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(hunk.header.clone())
        .into_any_element()
}

fn diff_line(line: &DiffLine, theme: &Theme) -> AnyElement {
    let (marker, background): (&str, Option<Hsla>) = match line.kind {
        LineKind::Context => (" ", None),
        LineKind::Added => ("+", Some(theme.success.opacity(0.15))),
        LineKind::Removed => ("-", Some(theme.error.opacity(0.15))),
    };
    let number = |n: Option<u32>| {
        div()
            .w(px(44.))
            .flex_none()
            .pr(px(6.))
            .flex()
            .justify_end()
            .text_color(theme.text_muted)
            .children(n.map(|n| n.to_string()))
    };
    let text: String = line.text.chars().take(MAX_LINE_CHARS).collect();
    let no_newline = line.no_newline_at_end.then(|| {
        div()
            .flex_none()
            .pl(px(8.))
            .text_color(theme.text_muted)
            .child("No newline at end of file")
    });
    div()
        .debug_selector(|| "diff-line".to_owned())
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .whitespace_nowrap()
        .overflow_hidden()
        .when_some(background, |row, background| row.bg(background))
        .child(number(line.old_line))
        .child(number(line.new_line))
        .child(div().w(px(16.)).flex_none().child(marker))
        .child(div().min_w_0().overflow_hidden().child(text))
        .children(no_newline)
        .into_any_element()
}
