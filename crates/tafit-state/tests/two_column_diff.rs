//! The two-column Line Diff and the changes highlighted within a line, read
//! through the store from real repos.

mod support;

use std::sync::Arc;

use support::{TestRepo, TestStore};
use tafit_core::{ColumnLine, FileDiff, WhitespaceMode};
use tafit_state::{
    AppState, ChangeGroup, DiffLayout, DiffOptions, FileRef, Loadable, Msg, RepoId, SelectMode,
    WindowId,
};

/// Opens `repo` in a fresh store with `options`, once its change list has loaded.
fn open(repo: &TestRepo, options: DiffOptions) -> (TestStore, RepoId) {
    let store = TestStore::new();
    store.dispatch(Msg::SetDiffOptions(options));
    store.dispatch(Msg::OpenRepo {
        window: WindowId::FIRST,
        path: repo.path().to_owned(),
    });
    let state = store.wait_until(|state| {
        state
            .active()
            .is_some_and(|repo| matches!(repo.status, Loadable::Ready(_)))
    });
    (store, state.active_repo().expect("the opened Repository"))
}

fn two_columns() -> DiffOptions {
    DiffOptions {
        layout: DiffLayout::TwoColumn,
        ..DiffOptions::default()
    }
}

/// Selects only the unstaged `path` and waits for a diff that `wanted` accepts.
fn diff_of(
    store: &TestStore,
    repo_id: RepoId,
    path: &str,
    wanted: impl Fn(&FileDiff) -> bool,
) -> Arc<FileDiff> {
    let file = FileRef {
        group: ChangeGroup::Unstaged,
        path: path.to_owned(),
    };
    store.dispatch(Msg::Select {
        repo_id,
        file: file.clone(),
        mode: SelectMode::Only,
    });
    store.flush();
    let state = store.wait_until(|state| loaded_diff(state, &file).is_some_and(|d| wanted(&d)));
    loaded_diff(&state, &file).expect("waited for it")
}

fn loaded_diff(state: &AppState, file: &FileRef) -> Option<Arc<FileDiff>> {
    let diff = state.active()?.diff.as_ref()?;
    match &diff.diff {
        Loadable::Ready(loaded) if diff.file == *file => Some(Arc::clone(loaded)),
        Loadable::Error(error) => panic!("the diff failed: {error}"),
        _ => None,
    }
}

fn is_two_column(diff: &FileDiff) -> bool {
    matches!(diff, FileDiff::TwoColumn(_))
}

/// Each row as `old | new`, trimmed; a side is `<marker><number> <text>`,
/// the marker `-`/`+` only on a changed line, or empty where the side has no
/// line.
fn rows(diff: &FileDiff) -> Vec<String> {
    let FileDiff::TwoColumn(rows) = diff else {
        panic!("expected a two-column diff, got {diff:?}");
    };
    let side = |line: &Option<ColumnLine>, marker: &str| match line {
        None => String::new(),
        Some(line) => {
            let marker = if line.changed { marker } else { "" };
            format!("{marker}{} {}", line.number, line.text)
        }
    };
    rows.iter()
        .map(|row| format!("{} | {}", side(&row.old, "-"), side(&row.new, "+")))
        .map(|row| row.trim().to_owned())
        .collect()
}

#[test]
fn a_moved_block_of_code_keeps_the_rest_of_the_file_aligned() {
    let a = "fn a() {\n    one();\n}\n";
    let b = "fn b() {\n    two();\n}\n";
    let c = "fn c() {\n    three();\n}\n";
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("lib.rs", &format!("{a}\n{b}\n{c}"))], "base");
    repo.write("lib.rs", &format!("{b}\n{c}\n{a}"));
    let (store, repo_id) = open(&repo, two_columns());

    let diff = diff_of(&store, repo_id, "lib.rs", is_two_column);

    assert_eq!(
        rows(&diff),
        [
            "-1 fn a() { |",
            "-2     one(); |",
            "-3 } |",
            "-4  |",
            "5 fn b() { | 1 fn b() {",
            "6     two(); | 2     two();",
            "7 } | 3 }",
            "8  | 4",
            "9 fn c() { | 5 fn c() {",
            "10     three(); | 6     three();",
            "11 } | 7 }",
            "| +8",
            "| +9 fn a() {",
            "| +10     one();",
            "| +11 }",
        ]
    );
}

/// The highlighted parts of a line's text.
fn highlighted<'a>(text: &'a str, highlights: &[std::ops::Range<usize>]) -> Vec<&'a str> {
    highlights
        .iter()
        .map(|range| &text[range.clone()])
        .collect()
}

/// The one row whose two sides are both changed: a line edited in place.
fn edited_row(diff: &FileDiff) -> (ColumnLine, ColumnLine) {
    let FileDiff::TwoColumn(rows) = diff else {
        panic!("expected a two-column diff, got {diff:?}");
    };
    let mut edited = rows.iter().filter_map(|row| match (&row.old, &row.new) {
        (Some(old), Some(new)) if old.changed && new.changed => Some((old.clone(), new.clone())),
        _ => None,
    });
    let row = edited.next().expect("an edited row");
    assert!(edited.next().is_none(), "only one edited row");
    row
}

#[test]
fn a_one_word_change_highlights_only_that_word() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.rs", "fn total() {\n    price * count\n}\n")], "base");
    repo.write("a.rs", "fn total() {\n    price * amount\n}\n");
    let (store, repo_id) = open(&repo, two_columns());

    let diff = diff_of(&store, repo_id, "a.rs", is_two_column);

    let (old, new) = edited_row(&diff);
    assert_eq!(highlighted(&old.text, &old.highlights), ["count"]);
    assert_eq!(highlighted(&new.text, &new.highlights), ["amount"]);
}

#[test]
fn the_inline_layout_highlights_the_changed_word_too() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.rs", "let unit = \"cm\";\n")], "base");
    repo.write("a.rs", "let unit = \"mm\";\n");
    let (store, repo_id) = open(&repo, DiffOptions::default());

    let diff = diff_of(&store, repo_id, "a.rs", |diff| {
        matches!(diff, FileDiff::Text(_))
    });

    let FileDiff::Text(hunks) = &*diff else {
        unreachable!("waited for a text diff")
    };
    let highlights: Vec<Vec<&str>> = hunks[0]
        .lines
        .iter()
        .map(|line| highlighted(&line.text, &line.highlights))
        .collect();
    assert_eq!(highlights, [vec!["cm"], vec!["mm"]]);
}

#[test]
fn lines_too_different_to_compare_highlight_all_between_their_common_ends() {
    let old = "call(alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi);";
    let new = "call(one two three four five six seven eight nine ten eleven twelve thirteen);";
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.rs", &format!("{old}\n"))], "base");
    repo.write("a.rs", &format!("{new}\n"));
    let (store, repo_id) = open(&repo, two_columns());

    let diff = diff_of(&store, repo_id, "a.rs", is_two_column);

    let (old, new) = edited_row(&diff);
    assert_eq!(
        highlighted(&old.text, &old.highlights),
        ["alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi"]
    );
    assert_eq!(
        highlighted(&new.text, &new.highlights),
        ["one two three four five six seven eight nine ten eleven twelve thirteen"]
    );
}

#[test]
fn ignoring_whitespace_keeps_a_reindented_line_unchanged_in_two_columns() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.rs", "if ok {\nrun();\n}\nlet x = 1;\n")], "base");
    repo.write("a.rs", "if ok {\n    run();\n}\nlet x = 2;\n");
    let options = DiffOptions {
        whitespace: WhitespaceMode::Ignore,
        ..two_columns()
    };
    let (store, repo_id) = open(&repo, options);

    let diff = diff_of(&store, repo_id, "a.rs", is_two_column);

    assert_eq!(
        rows(&diff),
        [
            "1 if ok { | 1 if ok {",
            "2 run(); | 2     run();",
            "3 } | 3 }",
            "-4 let x = 1; | +4 let x = 2;",
        ]
    );
}

#[test]
fn ignoring_whitespace_hides_a_whitespace_only_change_inline() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.rs", "run();\n")], "base");
    repo.write("a.rs", "    run( );\n");
    let ignore = DiffOptions {
        whitespace: WhitespaceMode::Ignore,
        ..DiffOptions::default()
    };
    let (store, repo_id) = open(&repo, ignore);

    let diff = diff_of(&store, repo_id, "a.rs", |diff| {
        matches!(diff, FileDiff::Text(_))
    });

    assert_eq!(*diff, FileDiff::Text(Vec::new()));

    store.dispatch(Msg::SetDiffOptions(DiffOptions::default()));
    store.flush();
    store.wait_until(|state| {
        state
            .active()
            .and_then(|repo| repo.diff.as_ref())
            .is_some_and(|diff| matches!(&diff.diff, Loadable::Ready(d) if matches!(&**d, FileDiff::Text(h) if !h.is_empty())))
    });
}

#[test]
fn ignoring_whitespace_never_highlights_spaces_within_an_edited_line() {
    let repo = TestRepo::init("repo");
    repo.commit_files(&[("a.rs", "x = a+b;\n")], "base");
    repo.write("a.rs", "x = a + c;\n");
    let options = DiffOptions {
        whitespace: WhitespaceMode::Ignore,
        ..two_columns()
    };
    let (store, repo_id) = open(&repo, options);

    let diff = diff_of(&store, repo_id, "a.rs", is_two_column);

    let (old, new) = edited_row(&diff);
    assert_eq!(highlighted(&old.text, &old.highlights), ["b"]);
    assert_eq!(highlighted(&new.text, &new.highlights), ["c"]);
}
