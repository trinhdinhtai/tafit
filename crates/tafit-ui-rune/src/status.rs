//! The change list of the Active Repository: staged, unstaged and untracked
//! files in one virtual list, so only the rows on screen are built. Below
//! it, the commit message box.

use std::sync::Arc;

use rune::{El, ScrollSpec, Tx, col, row, text};
use rune_kit::{Button, ButtonVariant, Kit, TextArea};
use tafit_core::{ChangeKind, FileChange, RepositoryStatus};
use tafit_state::{ChangeGroup, FileRef, Loadable, Msg, RepoState, SelectMode};

use crate::{Ui, centered};

const ROW_HEIGHT: f32 = 24.0;

pub(crate) fn status_panel(s: &Ui, repo: &RepoState, k: &Kit) -> El {
    let repo_id = repo.id;
    let refresh = Button::new(if repo.status_refreshing {
        "Refreshing…"
    } else {
        "Refresh"
    })
    .variant(ButtonVariant::Ghost)
    .on_press(move |s: &mut Ui| s.dispatch(Msg::RefreshStatus(repo_id)))
    .build(k)
    .key("status-refresh");
    let header = row()
        .w_full()
        .gap(6.0)
        .px(14.0)
        .py(6.0)
        .items_center()
        .child(text("Changes").font(k.ty.ui).color(k.c.text_muted))
        .child(crate::spacer())
        .child(write_button(repo, true, k))
        .child(write_button(repo, false, k))
        .child(refresh);
    let body = match &repo.status {
        Loadable::Loading => centered("Reading changes…", k.c.text_muted, k),
        Loadable::Error(message) => centered(message, k.c.danger.fg, k),
        Loadable::Ready(status) if status.is_clean() => {
            centered("No changes", k.c.text_muted, k).key("status-clean")
        }
        Loadable::Ready(status) => change_list(Arc::clone(status), k),
    };
    col()
        .key("status-panel")
        .w(380.0)
        .h_full()
        .min_h(0.0)
        .child(header)
        .child(body)
        .child(commit_box(s, repo, k))
}

/// The Stage (or Unstage) button for the selection; disabled when the
/// selection has nothing to stage (unstage) or a write is running.
fn write_button(repo: &RepoState, stage: bool, k: &Kit) -> El {
    let (label, key) = if stage {
        ("Stage", "stage-selected")
    } else {
        ("Unstage", "unstage-selected")
    };
    let applies = repo
        .selection
        .iter()
        .any(|file| (file.group == ChangeGroup::Staged) != stage);
    let repo_id = repo.id;
    Button::new(label)
        .variant(ButtonVariant::Outline)
        .disabled(repo.writing || !applies)
        .on_press(move |s: &mut Ui| {
            s.dispatch(if stage {
                Msg::StageSelected(repo_id)
            } else {
                Msg::UnstageSelected(repo_id)
            });
        })
        .build(k)
        .key(key)
}

/// The three groups flattened into rows: each non-empty group is a header
/// row followed by its files.
fn change_list(status: Arc<RepositoryStatus>, k: &Kit) -> El {
    let count = groups(&status)
        .iter()
        .filter(|group| !group.files.is_empty())
        .map(|group| group.files.len() + 1)
        .sum();
    let kit = k.clone();
    let rows = col()
        .key("status-rows")
        .w_full()
        .growth(rune::GrowthSpec::default())
        .virtual_rows(
            count,
            ROW_HEIGHT,
            |s: &Ui, i| row_key(s, i).into(),
            move |s: &Ui, i| match status_of(s).map(|status| row_at(status, i)) {
                Some(Row::Header(group)) => group_header(&group, &kit),
                Some(Row::File(group, file)) => file_row(s, group, file, &kit),
                Some(Row::Gone) | None => row(),
            },
        )
        // The rows' keys are the status's files: a new status is a new list.
        .virtual_rows_version(Arc::as_ptr(&status) as usize);
    col()
        .key("status-list")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(rows)
}

/// The shown status, which the virtual rows read on every build.
fn status_of(s: &Ui) -> Option<&RepositoryStatus> {
    match &s.state.active_in(s.window)?.status {
        Loadable::Ready(status) => Some(status),
        _ => None,
    }
}

fn row_key(s: &Ui, i: usize) -> String {
    match status_of(s).map(|status| row_at(status, i)) {
        Some(Row::Header(group)) => format!("group:{}", group.title),
        Some(Row::File(group, file)) => format!("{group:?}:{}", file.path),
        Some(Row::Gone) | None => format!("gone:{i}"),
    }
}

struct Group<'a> {
    group: ChangeGroup,
    title: &'static str,
    key: &'static str,
    files: &'a [FileChange],
}

fn groups(status: &RepositoryStatus) -> [Group<'_>; 3] {
    [
        (ChangeGroup::Staged, "Staged", "status-staged"),
        (ChangeGroup::Unstaged, "Unstaged", "status-unstaged"),
        (ChangeGroup::Untracked, "Untracked", "status-untracked"),
    ]
    .map(|(group, title, key)| Group {
        group,
        title,
        key,
        files: group.files(status),
    })
}

enum Row<'a> {
    Header(Group<'a>),
    File(ChangeGroup, &'a FileChange),
    /// Past the end: the list asked for a row of an older status.
    Gone,
}

/// Row `i` of the flattened list; constant time, so scrolling costs the
/// same at row ten and row ten thousand.
fn row_at(status: &RepositoryStatus, mut i: usize) -> Row<'_> {
    for group in groups(status) {
        if group.files.is_empty() {
            continue;
        }
        if i == 0 {
            return Row::Header(group);
        }
        if let Some(file) = group.files.get(i - 1) {
            return Row::File(group.group, file);
        }
        i -= group.files.len() + 1;
    }
    Row::Gone
}

fn group_header(group: &Group<'_>, k: &Kit) -> El {
    row()
        .key(group.key)
        .w_full()
        .h(ROW_HEIGHT)
        .px(14.0)
        .items_center()
        .fill(k.c.layer_01)
        .child(
            text(format!("{} ({})", group.title, group.files.len()))
                .font(k.ty.ui)
                .color(k.c.text_muted),
        )
}

/// A click selects the file, Ctrl/Cmd-click adds or removes it, and a
/// double-click stages it (or unstages a staged one).
fn file_row(s: &Ui, group: ChangeGroup, file: &FileChange, k: &Kit) -> El {
    let (letter, color, label) = match &file.kind {
        ChangeKind::Added => ("A", k.c.success.fg, file.path.clone()),
        ChangeKind::Modified => ("M", k.c.warning.fg, file.path.clone()),
        ChangeKind::Deleted => ("D", k.c.danger.fg, file.path.clone()),
        ChangeKind::Renamed { from } => ("R", k.c.text_accent, format!("{from} → {}", file.path)),
    };
    let target = FileRef {
        group,
        path: file.path.clone(),
    };
    let selected = s
        .state
        .active_in(s.window)
        .is_some_and(|repo| repo.selection.contains(&target));
    let double = target.clone();
    row()
        .w_full()
        .h(ROW_HEIGHT)
        .px(14.0)
        .gap(8.0)
        .items_center()
        .role("option")
        .label(label.clone())
        .checked(selected)
        .fill(if selected { k.c.bg_accent } else { k.c.bg_base })
        .hover(if selected {
            k.c.bg_accent
        } else {
            k.c.surface_hover
        })
        .on_press(move |s: &mut Ui| {
            let mode = if s.mods.cmd {
                SelectMode::Toggle
            } else {
                SelectMode::Only
            };
            s.select_file(target.clone(), mode);
        })
        .on_double_press(move |s: &mut Ui| {
            s.select_file(double.clone(), SelectMode::Only);
            if let Some(repo) = s.state.active_in(s.window) {
                let repo_id = repo.id;
                s.dispatch(if double.group == ChangeGroup::Staged {
                    Msg::UnstageSelected(repo_id)
                } else {
                    Msg::StageSelected(repo_id)
                });
            }
        })
        .child(text(letter).w(12.0).font(k.ty.mono).color(color))
        .child(
            text(label)
                .grow(1.0)
                .min_w(0.0)
                .font(k.ty.ui)
                .color(k.c.text_base)
                .truncate(),
        )
}

impl Ui {
    fn select_file(&self, file: FileRef, mode: SelectMode) {
        if let Some(repo) = self.state.active_in(self.window) {
            self.dispatch(Msg::Select {
                repo_id: repo.id,
                file,
                mode,
            });
        }
    }
}

/// The message box, the last write's error, and the commit button.
fn commit_box(s: &Ui, repo: &RepoState, k: &Kit) -> El {
    let repo_id = repo.id;
    let refusal = repo.commit_refusal();
    let commit = Button::new(if repo.writing {
        "Committing…"
    } else {
        "Commit"
    })
    .disabled(repo.writing || refusal.is_some())
    .on_press(move |s: &mut Ui| s.dispatch(Msg::Commit(repo_id)))
    .build(k)
    .key("commit-button");
    let hint = refusal.filter(|_| !repo.writing).map(|refusal| {
        text(refusal)
            .w_full()
            .font(k.ty.text_xsmall)
            .color(k.c.text_muted)
            .wrap()
    });
    // Shown whole: a hook's output can run long, so it scrolls.
    let error = repo.write_error.as_ref().map(|error| {
        col()
            .key("write-error")
            .w_full()
            .max_h(160.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                text(error)
                    .font(k.ty.mono)
                    .color(k.c.danger.fg)
                    .wrap()
                    .selectable(true),
            )
    });
    let message = TextArea::new(&s.commit)
        .label("Commit message")
        .placeholder("Commit message")
        .width(None)
        .rows(4)
        // The store freezes the message while it is committed.
        .disabled(repo.committing)
        .on_edit(|s: &mut Ui| &mut s.commit)
        .on_change(|s: &mut Ui, _: &mut Tx| s.edit_commit_message())
        // Ctrl/Cmd-Enter commits.
        .on_submit(move |s: &mut Ui, _: &mut Tx| s.dispatch(Msg::Commit(repo_id)))
        .build(k)
        .key("commit-message");
    col()
        .key("commit-box")
        .w_full()
        .gap(6.0)
        .padding(10.0)
        .fill(k.c.layer_01)
        .child(message)
        .children(error)
        .child(
            row()
                .w_full()
                .gap(8.0)
                .items_center()
                .child(col().grow(1.0).min_w(0.0).children(hint))
                .child(commit),
        )
}

impl Ui {
    /// Every edit of the message box goes to the store.
    fn edit_commit_message(&self) {
        if let Some(repo) = self.state.active_in(self.window) {
            self.dispatch(Msg::EditCommitMessage {
                repo_id: repo.id,
                message: self.commit.text.clone(),
            });
        }
    }

    /// Shows the Active Repository's message after a tab switch or a commit.
    /// The box takes the store's message only then, so a snapshot lagging
    /// behind typing can't undo keys.
    pub(crate) fn sync_commit_message(&mut self) {
        let Some(repo) = self.state.active_in(self.window) else {
            return;
        };
        let source = Some((repo.id, repo.commits_made));
        if self.commit_source == source {
            return;
        }
        self.commit_source = source;
        if self.commit.text != repo.commit_message {
            self.commit = rune::TextEdit::new(repo.commit_message.clone());
        }
    }
}
