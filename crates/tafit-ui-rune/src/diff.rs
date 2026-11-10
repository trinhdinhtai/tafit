//! The Line Diff of the selected file in `rune-kit`'s diff view, unified or
//! split, under a toolbar of display options and of what can be done with
//! the Line Selection (ADR-0006).
//!
//! The store's hunks reach `rune-kit` as a patch it parses, since it builds
//! its hunks itself. A range picked across the line numbers becomes the
//! store's Line Selection, and the store's Line Selection is what the view
//! shows as selected.

use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rune::{El, KeyEvent, Tx, col, row};
use rune_kit::diff::{self as kit, DiffMode, DiffView, LineRange, Side};
use rune_kit::{Button, ButtonVariant, Dialog, Kit, SegmentedControl, Switch};
use tafit_core::{
    FileDiff, Hunk, LineAction, LineKind, LineRef, LineSelection, LineSide, WhitespaceMode,
};
use tafit_state::{DiffLayout, DiffOptions, FileRef, Loadable, Msg, RepoState};

use crate::{Ui, centered};

/// What the diff view shows, kept with the window: `rune-kit`'s state of
/// the shown file, and the store's result it was built from.
#[derive(Default)]
pub(crate) struct ShownDiff {
    pub(crate) view: kit::DiffState,
    /// The file and result `view` holds; another result rebuilds it.
    source: Option<(FileRef, Arc<FileDiff>)>,
    /// When the selected file's diff started loading; `None` once it has.
    waiting_since: Option<Instant>,
    /// The diff has been loading for `READING_DELAY`: the pane says so.
    reading: bool,
}

/// How long a diff may take before the pane says it is reading it. Most
/// load well within it, so until then the pane keeps what it showed rather
/// than flashing a notice at every click.
const READING_DELAY: Duration = Duration::from_millis(200);

impl Ui {
    /// Brings the diff view in step with the snapshot.
    pub(crate) fn sync_diff(&mut self) {
        let loading = self
            .state
            .active_in(self.window)
            .and_then(|repo| repo.diff.as_ref())
            .is_some_and(|diff| matches!(diff.diff, Loadable::Loading));
        if loading {
            self.diff.waiting_since.get_or_insert_with(Instant::now);
        } else {
            self.diff.waiting_since = None;
            self.diff.reading = false;
        }
        let options = self.state.diff_options;
        let shown = self.state.active_in(self.window).and_then(|repo| {
            let diff = repo.diff.as_ref()?;
            match &diff.diff {
                Loadable::Ready(loaded) => {
                    Some((diff.file.clone(), Arc::clone(loaded), diff.lines.clone()))
                }
                _ => None,
            }
        });
        let Some((file, loaded, lines)) = shown else {
            self.diff.source = None;
            return;
        };
        let same = self
            .diff
            .source
            .as_ref()
            .is_some_and(|(f, held)| *f == file && Arc::ptr_eq(held, &loaded));
        if !same {
            let files = kit::parse(&patch_of(&file.path, &loaded));
            let same_file = self.diff.source.as_ref().is_some_and(|(f, _)| *f == file);
            if same_file {
                // A newer result of the same file keeps where it is scrolled.
                self.diff.view.set_files(files);
            } else {
                self.diff.view = kit::DiffState::new(files);
            }
            self.diff.source = Some((file, loaded));
        }
        self.diff.view.set_mode(match options.layout {
            DiffLayout::Inline => DiffMode::Unified,
            DiffLayout::TwoColumn => DiffMode::Split,
        });
        self.diff.view.set_wrap(options.word_wrap);
        let range = self
            .diff
            .view
            .files()
            .first()
            .and_then(|shown| range_of(&lines, shown));
        self.diff.view.set_selection(range);
    }

    /// Whether the loading diff just reached `READING_DELAY`, so the pane
    /// must be drawn again to say it is reading; until then, asks to be
    /// woken when it does.
    pub(crate) fn diff_wait_elapsed(&mut self, tx: &mut Tx) -> bool {
        let Some(since) = self.diff.waiting_since else {
            return false;
        };
        if self.diff.reading {
            return false;
        }
        let waited = since.elapsed();
        if waited >= READING_DELAY {
            self.diff.reading = true;
            return true;
        }
        tx.wake_after(READING_DELAY - waited);
        false
    }
}

pub(crate) fn diff_pane(s: &Ui, repo: &RepoState, k: &Kit, layer: &mut Vec<El>) -> El {
    let body = match &repo.diff {
        None => {
            let hint = if repo.selection.len() > 1 {
                format!("{} files selected", repo.selection.len())
            } else {
                "Select a file to see its changes".to_owned()
            };
            centered(hint, k.c.text_muted, k).key("diff-empty")
        }
        Some(diff) => match &diff.diff {
            Loadable::Loading if s.diff.reading => centered("Reading diff…", k.c.text_muted, k),
            // Briefly, the last diff shown stays (picking lines in it does
            // nothing), so a quick load swaps it for the new one without a
            // flash in between.
            Loadable::Loading if !s.diff.view.files().is_empty() => {
                DiffView::new(|s: &Ui| &s.diff.view, |s: &mut Ui| &mut s.diff.view)
                    .key("diff-lines")
                    .build(s, k)
            }
            Loadable::Loading => col().grow(1.0),
            Loadable::Error(message) => centered(message, k.c.danger.fg, k),
            Loadable::Ready(loaded) => match &**loaded {
                FileDiff::Binary => {
                    centered("Binary file: no line diff to show", k.c.text_muted, k)
                        .key("diff-binary")
                }
                FileDiff::Text(hunks) if hunks.is_empty() => no_changes(k),
                _ => DiffView::new(|s: &Ui| &s.diff.view, |s: &mut Ui| &mut s.diff.view)
                    .key("diff-lines")
                    .on_select(|s: &mut Ui, picked: Option<LineRange>, _: &mut Tx| {
                        s.select_range(picked);
                    })
                    .build(s, k),
            },
        },
    };
    if let Some(dialog) = discard_dialog(repo, k) {
        dialog.build(k, layer);
    }
    col()
        .key("diff-view")
        .grow(1.0)
        .min_w(0.0)
        .h_full()
        .min_h(0.0)
        .child(toolbar(s, repo, k))
        .child(body)
}

fn no_changes(k: &Kit) -> El {
    centered("No content changes", k.c.text_muted, k)
}

impl Ui {
    /// Makes the changed lines of `picked` the Line Selection.
    fn select_range(&self, picked: Option<LineRange>) {
        let Some(repo) = self.state.active_in(self.window) else {
            return;
        };
        let lines = match (picked, self.diff.view.files().first()) {
            (Some(range), Some(shown)) => lines_in(shown, range),
            _ => LineSelection::new(),
        };
        self.dispatch(Msg::SelectLines {
            repo_id: repo.id,
            lines,
        });
    }

    fn set_diff_options(&self, change: impl FnOnce(&mut DiffOptions)) {
        // The latest options the store has, so quick clicks all count.
        let mut options = self.store().snapshot().diff_options;
        change(&mut options);
        self.dispatch(Msg::SetDiffOptions(options));
    }
}

/// The display options, then what can be done with the Line Selection.
fn toolbar(s: &Ui, repo: &RepoState, k: &Kit) -> El {
    let options = s.state.diff_options;
    let split = options.layout == DiffLayout::TwoColumn;
    let layout = SegmentedControl::new(&["Unified", "Split"], Some(usize::from(split)))
        .width(Some(160.0))
        .on_pick(|s: &mut Ui, i, _: &mut Tx| {
            s.set_diff_options(|o| {
                o.layout = if i == 1 {
                    DiffLayout::TwoColumn
                } else {
                    DiffLayout::Inline
                };
            });
        })
        .build(k)
        .key("diff-layout");
    let ignore_whitespace = Switch::new(options.whitespace == WhitespaceMode::Ignore)
        .label("Ignore whitespace")
        .on_toggle(|s: &mut Ui| {
            s.set_diff_options(|o| {
                o.whitespace = match o.whitespace {
                    WhitespaceMode::Show => WhitespaceMode::Ignore,
                    WhitespaceMode::Ignore => WhitespaceMode::Show,
                };
            });
        })
        .build(k)
        .key("diff-ignore-whitespace");
    let wrap = Switch::new(options.word_wrap)
        .label("Wrap")
        .on_toggle(|s: &mut Ui| s.set_diff_options(|o| o.word_wrap = !o.word_wrap))
        .build(k)
        .key("diff-word-wrap");
    let line_actions = repo
        .diff
        .as_ref()
        .filter(|diff| !diff.lines.is_empty())
        .map(|diff| {
            let count = diff.lines.len();
            let (repo_id, group) = (repo.id, diff.file.group);
            row().gap(6.0).children(
                ACTIONS
                    .iter()
                    .filter(move |(action, ..)| group.allows(*action))
                    .map(move |&(action, verb, key)| {
                        Button::new(format!("{verb} {}", lines_label(count)))
                            .variant(ButtonVariant::Outline)
                            .disabled(repo.writing)
                            .on_press(move |s: &mut Ui| {
                                s.dispatch(Msg::ApplyLines { repo_id, action })
                            })
                            .build(k)
                            .key(key)
                    }),
            )
        });
    row()
        .key("diff-toolbar")
        .w_full()
        .gap(12.0)
        .px(8.0)
        .py(6.0)
        .items_center()
        .fill(k.c.layer_01)
        .child(layout)
        .child(ignore_whitespace)
        .child(wrap)
        .child(crate::spacer())
        .children(line_actions)
}

/// The actions on the Line Selection, by verb and key.
const ACTIONS: [(LineAction, &str, &str); 3] = [
    (LineAction::Stage, "Stage", "diff-stage-lines"),
    (LineAction::Unstage, "Unstage", "diff-unstage-lines"),
    (LineAction::Discard, "Discard", "diff-discard-lines"),
];

/// `count` lines, e.g. `1 line` or `3 lines`.
fn lines_label(count: usize) -> String {
    match count {
        1 => "1 line".to_owned(),
        _ => format!("{count} lines"),
    }
}

/// Asks to confirm the Discard waiting in `repo`, which git can't undo.
fn discard_dialog(repo: &RepoState, k: &Kit) -> Option<Dialog> {
    let pending = repo.confirm_discard.as_ref()?;
    let repo_id = repo.id;
    let actions = row()
        .w_full()
        .gap(8.0)
        .justify_end()
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Ghost)
                .on_press(move |s: &mut Ui| s.dispatch(Msg::CancelDiscard(repo_id)))
                .build(k)
                .key("discard-cancel"),
        )
        .child(
            Button::new("Discard")
                .variant(ButtonVariant::Danger)
                .on_press(move |s: &mut Ui| s.dispatch(Msg::ConfirmDiscard(repo_id)))
                .build(k)
                .key("discard-confirm"),
        );
    Some(
        Dialog::new(
            "discard-dialog",
            format!("Discard {}?", lines_label(pending.lines.len())),
        )
        .description(format!(
            "The changes to {} are removed from the work tree. This can't be undone.",
            pending.file.path
        ))
        .width(420.0)
        .fit()
        .open(true)
        .on_close(move |s: &mut Ui| s.dispatch(Msg::CancelDiscard(repo_id)))
        .child(actions),
    )
}

/// `j`/`k` and `]`/`[` move between hunks and files when nothing else took
/// the key.
pub(crate) fn on_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    s.diff.source.is_some() && s.diff.view.key(ev, tx)
}

/// `diff` of `path` as a patch `rune-kit` parses.
fn patch_of(path: &str, diff: &FileDiff) -> String {
    let mut patch = format!("--- a/{path}\n+++ b/{path}\n");
    match diff {
        FileDiff::Text(hunks) => {
            for hunk in hunks {
                write_hunk(&mut patch, hunk);
            }
        }
        FileDiff::Binary => {}
    }
    patch
}

fn write_hunk(patch: &mut String, hunk: &Hunk) {
    let _ = writeln!(patch, "{}", hunk.header);
    for line in &hunk.lines {
        let marker = match line.kind {
            LineKind::Context => ' ',
            LineKind::Added => '+',
            LineKind::Removed => '-',
        };
        let _ = writeln!(patch, "{marker}{}", line.text);
        if line.no_newline_at_end {
            patch.push_str("\\ No newline at end of file\n");
        }
    }
}

fn side_of(side: LineSide) -> Side {
    match side {
        LineSide::Old => Side::Old,
        LineSide::New => Side::New,
    }
}

/// The changed line `line` is, as the store names it.
fn line_ref(line: &kit::Line) -> Option<LineRef> {
    match line.kind {
        kit::LineKind::Removed => line.old.map(|number| LineRef {
            side: LineSide::Old,
            number,
        }),
        kit::LineKind::Added => line.new.map(|number| LineRef {
            side: LineSide::New,
            number,
        }),
        kit::LineKind::Context => None,
    }
}

/// The lines of `file` in the order the view shows them.
fn lines_of(file: &kit::FileDiff) -> impl Iterator<Item = &kit::Line> {
    file.hunks.iter().flat_map(|hunk| hunk.lines.iter())
}

/// Whether `line` is the one `at` points to.
fn is_at(line: &kit::Line, at: kit::LineRef) -> bool {
    line.number(at.side) == Some(at.line)
}

/// The changed lines from `range`'s start to its end, either way round.
fn lines_in(file: &kit::FileDiff, range: LineRange) -> LineSelection {
    let lines: Vec<&kit::Line> = lines_of(file).collect();
    let find = |at| lines.iter().position(|line| is_at(line, at));
    let (Some(a), Some(b)) = (find(range.start), find(range.end)) else {
        return LineSelection::new();
    };
    lines[a.min(b)..=a.max(b)]
        .iter()
        .filter_map(|line| line_ref(line))
        .collect()
}

/// The range from the first to the last of `lines` the view shows.
fn range_of(lines: &LineSelection, file: &kit::FileDiff) -> Option<LineRange> {
    let mut selected =
        lines_of(file).filter(|line| line_ref(line).is_some_and(|r| lines.contains(&r)));
    let first = selected.next()?;
    let last = selected.last().unwrap_or(first);
    let at = |line: &kit::Line| {
        let r = line_ref(line).expect("only changed lines are selected");
        kit::LineRef::new(side_of(r.side), r.number)
    };
    Some(LineRange {
        file: 0,
        start: at(first),
        end: at(last),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks() -> FileDiff {
        // `two` became `TWO`, and `four` was added.
        let patch = "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,4 @@\n one\n-two\n+TWO\n three\n+four\n";
        FileDiff::parse(patch.as_bytes())
    }

    fn old(number: u32) -> LineRef {
        LineRef {
            side: LineSide::Old,
            number,
        }
    }

    fn new(number: u32) -> LineRef {
        LineRef {
            side: LineSide::New,
            number,
        }
    }

    #[test]
    fn a_range_selects_the_changed_lines_inside_it() {
        let files = kit::parse(&patch_of("a.txt", &hunks()));
        let range = LineRange {
            file: 0,
            start: kit::LineRef::new(Side::Old, 1),
            end: kit::LineRef::new(Side::New, 3),
        };
        assert_eq!(
            lines_in(&files[0], range),
            [old(2), new(2)].into_iter().collect()
        );
    }

    #[test]
    fn the_line_selection_shows_as_the_range_around_it() {
        let files = kit::parse(&patch_of("a.txt", &hunks()));
        let lines = [new(2), new(4)].into_iter().collect();
        let range = range_of(&lines, &files[0]).expect("a range");
        assert_eq!(range.start, kit::LineRef::new(Side::New, 2));
        assert_eq!(range.end, kit::LineRef::new(Side::New, 4));
    }
}
