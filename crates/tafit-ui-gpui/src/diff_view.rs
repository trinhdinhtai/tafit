//! The Line Diff of the selected file, inline or in two columns, under a
//! toolbar of display options. Rows are virtualized, so a diff of a huge
//! file builds only the rows on screen.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    AnyElement, ColorExt, Context, Div, HighlightStyle, Hsla, IntoElement, ListAlignment,
    ListState, Stateful, StyledText, div, list, prelude::*, px,
};
use tafit_core::{ColumnLine, ColumnRow, DiffLine, FileDiff, Hunk, LineKind, WhitespaceMode};
use tafit_state::{DiffLayout, DiffOptions, FileRef, Loadable, Msg, RepoId, RepoState};
use tafit_ui_kit::{Theme, fonts, toggle_button};

use crate::{TafitView, centered_message};

const ROW_HEIGHT: f32 = 20.;

/// Characters of an unwrapped line drawn; shaping a minified file's one huge
/// line would stall every frame it is on screen.
// shortcut: the rest of a longer line is cut off until the diff scrolls
// sideways; word wrap shows more of it.
const MAX_LINE_CHARS: usize = 1_000;

/// Characters of a wrapped line drawn, for the same reason.
const MAX_WRAPPED_CHARS: usize = 20_000;

/// Pixels of rows built beyond the visible ones, so scrolling shows no gap.
const OVERDRAW: f32 = 200.;

/// The diff lists' scroll state, kept by the view across frames.
pub(crate) struct DiffLists {
    /// The Repository, file and layout the lists were made for; another one
    /// starts them over at the top.
    shown: Option<(RepoId, FileRef, DiffLayout)>,
    /// The result the lists hold rows for; a newer result of the same file
    /// swaps the rows in place, keeping the scroll position. Held, not just
    /// its address, so a new result can't reuse the address and look old.
    result: Option<Arc<FileDiff>>,
    /// The options last drawn with; a change re-measures the rows.
    drawn_with: Option<DiffOptions>,
    /// The inline list; or the two-column list, which is the old column's
    /// alone when the columns scroll on their own.
    main: ListState,
    /// The new column's list when the columns scroll on their own.
    new_column: ListState,
}

impl Default for DiffLists {
    fn default() -> Self {
        Self {
            shown: None,
            result: None,
            drawn_with: None,
            main: new_list(0),
            new_column: new_list(0),
        }
    }
}

fn new_list(rows: usize) -> ListState {
    ListState::new(rows, ListAlignment::Top, px(OVERDRAW))
}

impl DiffLists {
    /// The lists for `rows` rows of `diff`, `file`'s diff in `repo`.
    fn prepare(
        &mut self,
        repo: RepoId,
        file: &FileRef,
        diff: &Arc<FileDiff>,
        rows: usize,
        options: DiffOptions,
    ) -> (ListState, ListState) {
        let shown = (repo, file.clone(), options.layout);
        let same_result = self
            .result
            .as_ref()
            .is_some_and(|held| Arc::ptr_eq(held, diff));
        if self.shown.as_ref() != Some(&shown) {
            self.main = new_list(rows);
            self.new_column = new_list(rows);
        } else if !same_result {
            for list in [&self.main, &self.new_column] {
                list.splice(0..list.item_count(), rows);
            }
        } else if self.drawn_with != Some(options) {
            self.main.remeasure();
            self.new_column.remeasure();
        }
        self.shown = Some(shown);
        self.result = Some(Arc::clone(diff));
        self.drawn_with = Some(options);
        (self.main.clone(), self.new_column.clone())
    }
}

pub(crate) fn diff_view(
    repo: &RepoState,
    options: DiffOptions,
    lists: &mut DiffLists,
    theme: &Theme,
    cx: &mut Context<TafitView>,
) -> impl IntoElement {
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
                FileDiff::Text(hunks) if hunks.is_empty() => no_changes(theme),
                FileDiff::TwoColumn(rows) if rows.is_empty() => no_changes(theme),
                FileDiff::Text(hunks) => {
                    let rows = hunks.iter().map(|hunk| hunk.lines.len() + 1).sum();
                    let (list, _) = lists.prepare(repo.id, &diff.file, loaded, rows, options);
                    inline_lines(Arc::clone(loaded), list, options, *theme)
                }
                FileDiff::TwoColumn(rows) => {
                    let (old, new) =
                        lists.prepare(repo.id, &diff.file, loaded, rows.len(), options);
                    two_columns(Arc::clone(loaded), old, new, options, *theme)
                }
            },
        },
    };
    div()
        .debug_selector(|| "diff-view".to_owned())
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .border_l_1()
        .border_t_1()
        .border_color(theme.border)
        .child(toolbar(options, theme, cx))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .font_family(fonts::MONOSPACE_FONT_FAMILY)
                .text_xs()
                .child(body),
        )
}

fn no_changes(theme: &Theme) -> AnyElement {
    centered_message("No content changes", theme.text_muted).into_any_element()
}

/// The display options. A click changes the latest options the store has
/// published, so clicks quicker than the snapshots that show them all count.
fn toolbar(options: DiffOptions, theme: &Theme, cx: &mut Context<TafitView>) -> impl IntoElement {
    let two_columns = options.layout == DiffLayout::TwoColumn;
    let ignore_whitespace = options.whitespace == WhitespaceMode::Ignore;
    // Only two columns have anything to scroll together.
    let sync_scroll = if two_columns {
        Some(option_button(
            "diff-sync-scroll",
            "Sync scroll",
            options.sync_scroll,
            theme,
            cx,
            |o| o.sync_scroll = !o.sync_scroll,
        ))
    } else {
        None
    };
    div()
        .flex()
        .flex_none()
        .flex_wrap()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(4.))
        .border_b_1()
        .border_color(theme.border)
        .child(option_button(
            "diff-inline",
            "Inline",
            !two_columns,
            theme,
            cx,
            |o| {
                o.layout = DiffLayout::Inline;
            },
        ))
        .child(option_button(
            "diff-two-column",
            "Two columns",
            two_columns,
            theme,
            cx,
            |o| {
                o.layout = DiffLayout::TwoColumn;
            },
        ))
        .child(option_button(
            "diff-ignore-whitespace",
            "Ignore whitespace",
            ignore_whitespace,
            theme,
            cx,
            |o| {
                o.whitespace = match o.whitespace {
                    WhitespaceMode::Show => WhitespaceMode::Ignore,
                    WhitespaceMode::Ignore => WhitespaceMode::Show,
                };
            },
        ))
        .child(option_button(
            "diff-word-wrap",
            "Wrap",
            options.word_wrap,
            theme,
            cx,
            |o| {
                o.word_wrap = !o.word_wrap;
            },
        ))
        .child(option_button(
            "diff-tab-size",
            format!("Tab size: {}", options.tab_size),
            false,
            theme,
            cx,
            |o| {
                o.tab_size = match o.tab_size {
                    2 => 4,
                    4 => 8,
                    _ => 2,
                };
            },
        ))
        .children(sync_scroll)
}

fn option_button(
    selector: &'static str,
    label: impl Into<gpui::SharedString>,
    on: bool,
    theme: &Theme,
    cx: &mut Context<TafitView>,
    change: fn(&mut DiffOptions),
) -> Stateful<Div> {
    toggle_button(label, on, theme)
        .id(selector)
        .debug_selector(move || selector.to_owned())
        .on_click(cx.listener(move |view, _, _, _| {
            let mut options = view.store.snapshot().diff_options;
            change(&mut options);
            view.store.dispatch(Msg::SetDiffOptions(options));
        }))
}

fn inline_lines(
    diff: Arc<FileDiff>,
    state: ListState,
    options: DiffOptions,
    theme: Theme,
) -> AnyElement {
    let FileDiff::Text(hunks) = &*diff else {
        unreachable!("only inline diffs have hunks")
    };
    // The row each hunk's header is on; a hunk is its header and its lines.
    let mut starts = Vec::with_capacity(hunks.len());
    let mut row_count = 0;
    for hunk in hunks {
        starts.push(row_count);
        row_count += hunk.lines.len() + 1;
    }
    list(state, move |ix, _window, _cx| {
        let FileDiff::Text(hunks) = &*diff else {
            unreachable!("checked above")
        };
        // The last hunk starting at or before `ix`.
        let hunk = starts.partition_point(|&start| start <= ix) - 1;
        match ix - starts[hunk] {
            0 => hunk_header(&hunks[hunk], &theme),
            line => diff_line(&hunks[hunk].lines[line - 1], options, &theme),
        }
    })
    .size_full()
    .into_any_element()
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

fn diff_line(line: &DiffLine, options: DiffOptions, theme: &Theme) -> AnyElement {
    let (marker, colors) = match line.kind {
        LineKind::Context => (" ", None),
        LineKind::Added => ("+", Some(ChangeColors::added(theme))),
        LineKind::Removed => ("-", Some(ChangeColors::removed(theme))),
    };
    row_box(options)
        .debug_selector(|| "diff-line".to_owned())
        .when_some(colors, |row, colors| row.bg(colors.line))
        .child(line_number(line.old_line, theme))
        .child(line_number(line.new_line, theme))
        .child(div().w(px(16.)).flex_none().child(marker))
        .child(line_text(
            &line.text,
            &line.highlights,
            colors.map(|colors| colors.highlight),
            options,
        ))
        .children(no_newline(line.no_newline_at_end, theme))
        .into_any_element()
}

fn two_columns(
    diff: Arc<FileDiff>,
    old: ListState,
    new: ListState,
    options: DiffOptions,
    theme: Theme,
) -> AnyElement {
    if options.sync_scroll {
        // One list of whole rows: both sides of a row always share a height.
        return list(old, move |ix, _window, _cx| {
            let row = &rows(&diff)[ix];
            div()
                .debug_selector(|| "diff-column-row".to_owned())
                .flex()
                .child(column_side(row.old.as_ref(), Side::Old, options, &theme))
                .child(div().w(px(1.)).flex_none().bg(theme.border))
                .child(column_side(row.new.as_ref(), Side::New, options, &theme))
                .into_any_element()
        })
        .size_full()
        .into_any_element();
    }
    let column = |selector: &'static str, state: ListState, side: Side| {
        let diff = Arc::clone(&diff);
        div()
            .debug_selector(move || selector.to_owned())
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                list(state, move |ix, _window, _cx| {
                    let row = &rows(&diff)[ix];
                    let line = match side {
                        Side::Old => row.old.as_ref(),
                        Side::New => row.new.as_ref(),
                    };
                    div()
                        .debug_selector(|| "diff-column-row".to_owned())
                        .flex()
                        .child(column_side(line, side, options, &theme))
                        .into_any_element()
                })
                .size_full(),
            )
    };
    div()
        .size_full()
        .flex()
        .child(column("diff-old-column", old, Side::Old))
        .child(div().w(px(1.)).flex_none().bg(theme.border))
        .child(column("diff-new-column", new, Side::New))
        .into_any_element()
}

fn rows(diff: &FileDiff) -> &[ColumnRow] {
    match diff {
        FileDiff::TwoColumn(rows) => rows,
        _ => unreachable!("only two-column diffs have rows"),
    }
}

#[derive(Clone, Copy)]
enum Side {
    Old,
    New,
}

/// One side of a two-column row; `None` is a gap where the other side's
/// line has no counterpart.
fn column_side(line: Option<&ColumnLine>, side: Side, options: DiffOptions, theme: &Theme) -> Div {
    let Some(line) = line else {
        return row_box(options)
            .flex_1()
            .min_w_0()
            .bg(theme.surface.opacity(0.6));
    };
    let colors = line.changed.then(|| match side {
        Side::Old => ChangeColors::removed(theme),
        Side::New => ChangeColors::added(theme),
    });
    row_box(options)
        .flex_1()
        .min_w_0()
        .when_some(colors, |row, colors| row.bg(colors.line))
        .child(line_number(Some(line.number), theme))
        .child(line_text(
            &line.text,
            &line.highlights,
            colors.map(|colors| colors.highlight),
            options,
        ))
        .children(no_newline(line.no_newline_at_end, theme))
}

/// A changed line's background, and the stronger one of its changed words.
#[derive(Clone, Copy)]
struct ChangeColors {
    line: Hsla,
    highlight: Hsla,
}

impl ChangeColors {
    fn added(theme: &Theme) -> Self {
        Self {
            line: theme.success.opacity(0.15),
            highlight: theme.success.opacity(0.4),
        }
    }

    fn removed(theme: &Theme) -> Self {
        Self {
            line: theme.error.opacity(0.15),
            highlight: theme.error.opacity(0.4),
        }
    }
}

/// A line's row: one line tall, or as tall as its wrapped text.
fn row_box(options: DiffOptions) -> Div {
    let row = div().flex().min_w_0();
    if options.word_wrap {
        row.min_h(px(ROW_HEIGHT)).items_start().py(px(2.))
    } else {
        row.h(px(ROW_HEIGHT))
            .items_center()
            .whitespace_nowrap()
            .overflow_hidden()
    }
}

fn line_number(number: Option<u32>, theme: &Theme) -> impl IntoElement {
    div()
        .w(px(44.))
        .flex_none()
        .pr(px(6.))
        .flex()
        .justify_end()
        .text_color(theme.text_muted)
        .children(number.map(|n| n.to_string()))
}

fn no_newline(no_newline_at_end: bool, theme: &Theme) -> Option<impl IntoElement> {
    no_newline_at_end.then(|| {
        div()
            .flex_none()
            .pl(px(8.))
            .text_color(theme.text_muted)
            .child("No newline at end of file")
    })
}

/// A line's text with its tabs expanded and its changed words, if
/// `highlight` is given, on a stronger background.
fn line_text(
    text: &str,
    highlights: &[Range<usize>],
    highlight: Option<Hsla>,
    options: DiffOptions,
) -> impl IntoElement {
    let limit = if options.word_wrap {
        MAX_WRAPPED_CHARS
    } else {
        MAX_LINE_CHARS
    };
    let (shown, offsets) = expand_tabs(text, usize::from(options.tab_size.max(1)), limit);
    let runs: Vec<(Range<usize>, HighlightStyle)> = match highlight {
        Some(color) => highlights
            .iter()
            .map(|range| offsets[range.start]..offsets[range.end])
            .filter(|range| !range.is_empty())
            .map(|range| {
                let style = HighlightStyle {
                    background_color: Some(color),
                    ..HighlightStyle::default()
                };
                (range, style)
            })
            .collect(),
        None => Vec::new(),
    };
    div()
        .min_w_0()
        .flex_1()
        .child(StyledText::new(shown).with_highlights(runs))
}

/// `text`'s first `limit` characters with each tab turned into the spaces up
/// to the next multiple of `tab_size`, and for each byte offset of `text`
/// that starts a character (and its end), where it is in the result; offsets
/// past the cut map to the result's end.
fn expand_tabs(text: &str, tab_size: usize, limit: usize) -> (String, Vec<usize>) {
    let mut shown = String::with_capacity(text.len().min(limit));
    let mut offsets = Vec::with_capacity(text.len() + 1);
    let mut column = 0;
    for (at, c) in text.char_indices().take(limit) {
        offsets.resize(at + 1, shown.len());
        if c == '\t' {
            let spaces = tab_size - column % tab_size;
            shown.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            shown.push(c);
            column += 1;
        }
    }
    offsets.resize(text.len() + 1, shown.len());
    (shown, offsets)
}
