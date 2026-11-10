//! The Active Repository's history: every commit in one virtual list, so
//! only the rows on screen are built, with the refs on each commit and the
//! columns the user chose. The list tells the store which rows are on
//! screen; the store reads what is shown about them a window at a time.

use std::ops::Range;
use std::sync::Arc;

use rune::{El, ScrollSpec, col, row, text};
use rune_kit::{Badge, BadgeVariant, Button, ButtonVariant, Checkbox, Kit, ScrollBar};
use tafit_core::{CommitTime, RefKind, RefLabel};
use tafit_state::{HistoryColumns, HistoryState, Loadable, Msg, RepoId, RepoState, history_window};

use crate::{Ui, centered};

const ROW_HEIGHT: f32 = 24.0;
const AUTHOR_WIDTH: f32 = 160.0;
const DATE_WIDTH: f32 = 128.0;
const HASH_WIDTH: f32 = 64.0;
/// Hex digits of a commit id shown, as `git log --abbrev-commit` mostly does.
const SHORT_HASH: usize = 7;

pub(crate) fn history_panel(s: &Ui, repo: &RepoState, k: &Kit) -> El {
    let history = &repo.history;
    let body = match &history.index {
        Loadable::Loading => centered("Reading history…", k.c.text_muted, k),
        Loadable::Error(message) => centered(message, k.c.danger.fg, k).key("history-error"),
        Loadable::Ready(index) if index.is_empty() => {
            centered("No commits yet", k.c.text_muted, k).key("history-empty")
        }
        Loadable::Ready(index) => commit_list(history, Arc::as_ptr(index) as usize, k),
    };
    col()
        .key("history-panel")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(header(s, repo, k))
        .child(body)
}

/// The commit count, the column choice, and a refresh button.
fn header(s: &Ui, repo: &RepoState, k: &Kit) -> El {
    let history = &repo.history;
    let count = match &history.index {
        Loadable::Ready(index) => format!("{} commits", index.len()),
        _ => String::new(),
    };
    let columns = s.state.history_columns;
    let toggle = |label: &str, key: &str, on: bool, flip: fn(&mut HistoryColumns)| {
        Checkbox::new(on)
            .label(label)
            .on_toggle(move |s: &mut Ui| {
                let mut columns = s.state.history_columns;
                flip(&mut columns);
                s.dispatch(Msg::SetHistoryColumns(columns));
            })
            .build(k)
            .key(key)
    };
    let repo_id = repo.id;
    let refresh = Button::new(if history.refreshing {
        "Refreshing…"
    } else {
        "Refresh"
    })
    .variant(ButtonVariant::Ghost)
    .disabled(history.refreshing)
    .on_press(move |s: &mut Ui| s.dispatch(Msg::RefreshHistory(repo_id)))
    .build(k)
    .key("refresh-history");
    row()
        .w_full()
        .gap(12.0)
        .px(14.0)
        .py(6.0)
        .items_center()
        .child(text("History").font(k.ty.ui).color(k.c.text_muted))
        .child(
            text(count)
                .key("history-count")
                .font(k.ty.ui)
                .color(k.c.text_muted),
        )
        .child(crate::spacer())
        .child(toggle(
            "Author",
            "history-column-author",
            columns.author,
            |c| {
                c.author = !c.author;
            },
        ))
        .child(toggle("Date", "history-column-date", columns.date, |c| {
            c.date = !c.date;
        }))
        .child(toggle("Hash", "history-column-hash", columns.hash, |c| {
            c.hash = !c.hash;
        }))
        .child(refresh)
}

/// One row per commit. `version` identifies the index: the rows' keys
/// change only with it.
fn commit_list(history: &HistoryState, version: usize, k: &Kit) -> El {
    let kit = k.clone();
    let rows = col()
        .key("history-rows")
        .w_full()
        .growth(rune::GrowthSpec::default())
        .virtual_rows(
            history.len(),
            ROW_HEIGHT,
            |s: &Ui, i| row_key(s, i).into(),
            move |s: &Ui, i| match history_of(s) {
                Some(history) => commit_row(history, s.state.history_columns, i, &kit),
                None => row(),
            },
        )
        .virtual_rows_version(version)
        .on_visible_rows(|s: &mut Ui, rows| {
            s.show_history_rows(rows);
            false
        });
    col()
        .key("history-list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(rows)
        .scroll_bar(k)
}

/// The shown history, which the virtual rows read on every build.
fn history_of(s: &Ui) -> Option<&HistoryState> {
    Some(&s.state.active_in(s.window)?.history)
}

fn row_key(s: &Ui, i: usize) -> String {
    match history_of(s).and_then(|history| history.commit_id(i)) {
        Some(id) => id.to_string(),
        None => format!("gone:{i}"),
    }
}

fn commit_row(history: &HistoryState, columns: HistoryColumns, i: usize, k: &Kit) -> El {
    let Some(id) = history.commit_id(i) else {
        return row();
    };
    let commit = history.commit(i);
    let summary = match (commit, history.commit_error(i)) {
        (Some(commit), _) => text(&commit.summary).color(k.c.text_base),
        (None, Some(error)) => text(error).color(k.c.danger.fg),
        // Its window is being read.
        (None, None) => text("…").color(k.c.text_muted),
    };
    let cell = |value: String, width: f32| {
        text(value)
            .w(width)
            .font(k.ty.ui)
            .color(k.c.text_muted)
            .truncate()
    };
    let author = columns.author.then(|| {
        cell(
            commit.map_or_else(String::new, |c| c.author_name.clone()),
            AUTHOR_WIDTH,
        )
    });
    let date = columns.date.then(|| {
        cell(
            commit.map_or_else(String::new, |c| format_time(c.author_time)),
            DATE_WIDTH,
        )
    });
    let hash = columns.hash.then(|| {
        text(id.short(SHORT_HASH))
            .w(HASH_WIDTH)
            .font(k.ty.mono)
            .color(k.c.text_muted)
    });
    row()
        .w_full()
        .h(ROW_HEIGHT)
        .px(14.0)
        .gap(12.0)
        .items_center()
        .role("row")
        .label(commit.map_or_else(|| id.to_string(), |c| c.summary.clone()))
        .hover(k.c.surface_hover)
        .child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .gap(6.0)
                .items_center()
                .children(history.labels(i).iter().map(|label| ref_badge(label, k)))
                .child(summary.grow(1.0).min_w(0.0).font(k.ty.ui).truncate()),
        )
        .children(author)
        .children(date)
        .children(hash)
}

/// HEAD and its branch stand out; remote branches are quieter than local
/// ones; tags are outlined.
fn ref_badge(label: &RefLabel, k: &Kit) -> El {
    let badge = Badge::new(&label.name);
    let badge = match label.kind {
        _ if label.head => badge.variant(BadgeVariant::Accent),
        RefKind::Head | RefKind::Branch => badge,
        RefKind::RemoteBranch => badge.variant(BadgeVariant::Subtle),
        RefKind::Tag => badge.high_contrast(),
    };
    badge.build(k)
}

/// `YYYY-MM-DD HH:MM` in the author's own time zone, as git shows dates
/// with `--date=iso` minus seconds and offset.
fn format_time(time: CommitTime) -> String {
    let local = time.seconds + i64::from(time.offset);
    let (days, secs) = (local.div_euclid(86_400), local.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

impl Ui {
    /// Tells the store the rows on screen, when they reach another window
    /// than last told: the store reads by window, so scrolling within one
    /// sends nothing.
    fn show_history_rows(&mut self, rows: Range<usize>) {
        let Some(repo) = self.state.active_in(self.window) else {
            return;
        };
        let windows = (
            repo.id,
            history_window(rows.start),
            history_window(rows.end.saturating_sub(1)),
        );
        if self.history_windows == Some(windows) {
            return;
        }
        self.history_windows = Some(windows);
        let repo_id: RepoId = repo.id;
        self.dispatch(Msg::ShowHistoryRows { repo_id, rows });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_show_in_the_authors_time_zone() {
        let time = |seconds, offset| format_time(CommitTime { seconds, offset });
        assert_eq!(time(0, 0), "1970-01-01 00:00");
        // 2024-02-29 23:30 UTC, a leap day.
        assert_eq!(time(1_709_249_400, 0), "2024-02-29 23:30");
        assert_eq!(time(1_709_249_400, 7 * 3600), "2024-03-01 06:30");
        assert_eq!(time(1_709_249_400, -5 * 3600), "2024-02-29 18:30");
        assert_eq!(time(-1, 0), "1969-12-31 23:59");
    }
}
