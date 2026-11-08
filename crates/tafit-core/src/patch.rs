//! Patches built from a Line Selection, for `git apply` to stage, unstage or
//! Discard just those lines. Built from `git diff`'s own bytes, so CRLF
//! line endings and lines that aren't UTF-8 go back exactly as they came.

use std::collections::BTreeSet;

use crate::{Hunk, LineKind};

/// Which side of a diff a line number counts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LineSide {
    /// The diff's old side; a removed line is numbered here.
    Old,
    /// The diff's new side; an added line is numbered here.
    New,
}

/// One changed line of a file's diff: a removed line by its number on the
/// old side, an added line by its number on the new side. The same in every
/// layout and context size, since the numbers are the files' own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LineRef {
    pub side: LineSide,
    /// 1-based.
    pub number: u32,
}

/// The changed lines a user picked in one file's diff, possibly across
/// several hunks.
pub type LineSelection = BTreeSet<LineRef>;

/// What to do with a Line Selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineAction {
    /// Copy the selected changes of the unstaged diff into the index.
    Stage,
    /// Take the selected changes of the staged diff out of the index.
    Unstage,
    /// Undo the selected changes of the unstaged diff in the work tree.
    Discard,
}

impl LineAction {
    /// The patch undoes the diff's changes (applied with `--reverse`)
    /// rather than making them.
    pub fn reverses(self) -> bool {
        !matches!(self, Self::Stage)
    }
}

/// Why a Line Selection can't be applied.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LinePatchError {
    /// The file's diff is no longer the one the lines were selected in.
    #[error(
        "The file changed after the lines were selected. Check the diff and select them again."
    )]
    Stale,
    /// No changed line is selected.
    #[error("No changed line is selected.")]
    Empty,
    /// A selected line isn't a change in git's own diff of the file, e.g.
    /// one tafit's two-column alignment shows as changed but git doesn't.
    #[error(
        "Some selected lines aren't changes in git's diff of the file. Select them in the inline layout."
    )]
    Unmapped,
}

/// One line of the patch being built.
#[derive(Clone, Copy)]
struct PatchLine<'a> {
    kind: LineKind,
    /// Without the marker and the `\n`; a CRLF line keeps its `\r`.
    bytes: &'a [u8],
    no_newline_at_end: bool,
}

/// A hunk of `git diff` output, kept as bytes.
struct RawHunk<'a> {
    hunk: Hunk,
    /// Each line with its numbers (old, new) as `FileDiff::parse` gives them.
    lines: Vec<(PatchLine<'a>, Option<u32>, Option<u32>)>,
}

/// Parses one file's `git diff` output into its header lines (everything
/// before the first hunk) and its hunks.
fn parse(output: &[u8]) -> (Vec<&[u8]>, Vec<RawHunk<'_>>) {
    let mut header = Vec::new();
    let mut hunks: Vec<RawHunk> = Vec::new();
    let (mut old_left, mut new_left) = (0u32, 0u32);
    let (mut old_next, mut new_next) = (0u32, 0u32);
    for raw in output.split(|&byte| byte == b'\n') {
        if raw.starts_with(b"\\") {
            if let Some((line, ..)) = hunks.last_mut().and_then(|hunk| hunk.lines.last_mut()) {
                line.no_newline_at_end = true;
            }
        } else if old_left > 0 || new_left > 0 {
            let (kind, bytes) = match raw.split_first() {
                Some((b'+', bytes)) => (LineKind::Added, bytes),
                Some((b'-', bytes)) => (LineKind::Removed, bytes),
                Some((_, bytes)) => (LineKind::Context, bytes),
                None => (LineKind::Context, raw),
            };
            let (mut old_line, mut new_line) = (None, None);
            if kind != LineKind::Added {
                old_line = Some(old_next);
                old_next += 1;
                old_left = old_left.saturating_sub(1);
            }
            if kind != LineKind::Removed {
                new_line = Some(new_next);
                new_next += 1;
                new_left = new_left.saturating_sub(1);
            }
            let line = PatchLine {
                kind,
                bytes,
                no_newline_at_end: false,
            };
            let hunk = hunks.last_mut().expect("counts are set by a hunk header");
            hunk.lines.push((line, old_line, new_line));
        } else if let Some(hunk) = Hunk::parse_header(raw) {
            (old_left, new_left) = (hunk.old_count, hunk.new_count);
            (old_next, new_next) = (hunk.old_start, hunk.new_start);
            hunks.push(RawHunk {
                hunk,
                lines: Vec::new(),
            });
        } else if hunks.is_empty() && !raw.is_empty() {
            header.push(raw);
        }
    }
    (header, hunks)
}

/// Builds the patch that applies `lines` of `output`, one file's `git diff`
/// (with context, and whitespace changes shown), for `git apply`; with
/// an `action` that reverses, for `git apply --reverse`.
///
/// Forward, an unselected removed line stays as context and an unselected
/// added line is left out; reversed, the other way round, so the patch's
/// side that must match the file always does.
pub fn line_patch(
    output: &[u8],
    lines: &LineSelection,
    action: LineAction,
) -> Result<Vec<u8>, LinePatchError> {
    if lines.is_empty() {
        return Err(LinePatchError::Empty);
    }
    let (header, hunks) = parse(output);
    let reverses = action.reverses();
    let mut found = 0;
    let mut every_change = true;
    let mut body = Vec::new();
    // How many lines the new side has gained over the old so far.
    let mut delta: i64 = 0;
    for raw in &hunks {
        let mut built = Vec::with_capacity(raw.lines.len());
        let mut any = false;
        for &(line, old_line, new_line) in &raw.lines {
            let selected = match line.kind {
                LineKind::Context => false,
                LineKind::Removed => old_line.is_some_and(|number| {
                    lines.contains(&LineRef {
                        side: LineSide::Old,
                        number,
                    })
                }),
                LineKind::Added => new_line.is_some_and(|number| {
                    lines.contains(&LineRef {
                        side: LineSide::New,
                        number,
                    })
                }),
            };
            if selected {
                found += 1;
                any = true;
                built.push(line);
                continue;
            }
            if line.kind != LineKind::Context {
                every_change = false;
            }
            match (line.kind, reverses) {
                (LineKind::Context, _) | (LineKind::Removed, false) | (LineKind::Added, true) => {
                    built.push(PatchLine {
                        kind: LineKind::Context,
                        ..line
                    });
                }
                _ => {}
            }
        }
        if !any {
            continue;
        }
        let built = fix_line_ends(built);
        let count = |kind| {
            built
                .iter()
                .filter(|line: &&PatchLine| line.kind == LineKind::Context || line.kind == kind)
                .count() as i64
        };
        let (old_count, new_count) = (count(LineKind::Removed), count(LineKind::Added));
        // The side that keeps every line of the file keeps its start; the
        // other is moved by what the hunks before it changed.
        let (old_start, new_start) = if reverses {
            let new_start = i64::from(raw.hunk.new_start);
            let first = first_line(new_start, i64::from(raw.hunk.new_count)) - delta;
            (header_start(first, old_count), new_start)
        } else {
            let old_start = i64::from(raw.hunk.old_start);
            let first = first_line(old_start, i64::from(raw.hunk.old_count)) + delta;
            (old_start, header_start(first, new_count))
        };
        delta += new_count - old_count;
        body.extend_from_slice(
            format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n").as_bytes(),
        );
        for line in built {
            body.push(match line.kind {
                LineKind::Context => b' ',
                LineKind::Added => b'+',
                LineKind::Removed => b'-',
            });
            body.extend_from_slice(line.bytes);
            body.push(b'\n');
            if line.no_newline_at_end {
                body.extend_from_slice(b"\\ No newline at end of file\n");
            }
        }
    }
    if found < lines.len() {
        return Err(LinePatchError::Unmapped);
    }
    let mut patch = file_header(&header, every_change, action);
    patch.extend(body);
    Ok(patch)
}

/// The first line a hunk side covers; a side with no lines starts at the
/// line before.
fn first_line(start: i64, count: i64) -> i64 {
    if count == 0 { start + 1 } else { start }
}

/// The start a hunk side gives in its header when it covers `count` lines
/// from `first` on; the inverse of `first_line`.
fn header_start(first: i64, count: i64) -> i64 {
    if count == 0 { first - 1 } else { first }
}

/// Fixes the lines that end a file without a newline but are no longer
/// last on their side of the patch, as when an unselected line turns into
/// context, or a selected removed one is restored before lines kept. Such
/// a line gets its newline back on each side with lines after it; context
/// that keeps it on one side only splits into a removed and an added line.
/// Removed lines then go before added ones in each run of changes.
fn fix_line_ends(lines: Vec<PatchLine<'_>>) -> Vec<PatchLine<'_>> {
    let mut fixed = Vec::with_capacity(lines.len() + 1);
    for (ix, &line) in lines.iter().enumerate() {
        let later = &lines[ix + 1..];
        let old_later = later.iter().any(|later| later.kind != LineKind::Added);
        let new_later = later.iter().any(|later| later.kind != LineKind::Removed);
        if !line.no_newline_at_end {
            fixed.push(line);
            continue;
        }
        match line.kind {
            LineKind::Removed => fixed.push(PatchLine {
                no_newline_at_end: !old_later,
                ..line
            }),
            LineKind::Added => fixed.push(PatchLine {
                no_newline_at_end: !new_later,
                ..line
            }),
            LineKind::Context if old_later || new_later => {
                for (kind, more) in [(LineKind::Removed, old_later), (LineKind::Added, new_later)] {
                    fixed.push(PatchLine {
                        kind,
                        no_newline_at_end: !more,
                        ..line
                    });
                }
            }
            LineKind::Context => fixed.push(line),
        }
    }
    for run in fixed.split_mut(|line| line.kind == LineKind::Context) {
        run.sort_by_key(|line| line.kind != LineKind::Removed);
    }
    fixed
}

/// Header lines a line patch never carries: it changes a file's content
/// only, never its mode or name.
const DROPPED_HEADER_LINES: &[&[u8]] = &[
    b"old mode",
    b"new mode",
    b"similarity index",
    b"dissimilarity index",
    b"rename from",
    b"rename to",
    b"copy from",
    b"copy to",
];

const NO_FILE: &[u8] = b"/dev/null";

/// The diff's file header, made to fit a patch of some of its lines.
///
/// It names the file as it is now on both sides, so a rename stays as it
/// is, and leaves modes alone. A file the diff creates stays a file when
/// the patch undoes only some of its lines, or undoes them in the work
/// tree (a Discard empties a file added with intent, never deletes it); a
/// file the diff deletes stays one when only some of its lines go.
fn file_header(header: &[&[u8]], every_change: bool, action: LineAction) -> Vec<u8> {
    let has = |prefix: &[u8]| header.iter().any(|line| line.starts_with(prefix));
    let reverses = action.reverses();
    let keep_file =
        (has(b"new file mode") && reverses && (!every_change || action == LineAction::Discard))
            || (has(b"deleted file mode") && !reverses && !every_change);
    let renamed = has(b"rename from") || has(b"copy from");
    let name_after = |prefix: &[u8]| {
        header
            .iter()
            .find_map(|line| line.strip_prefix(prefix))
            .unwrap_or_default()
    };
    // The file's name where it exists: after the change, unless deleted.
    let name = match name_after(b"+++ ") {
        NO_FILE => name_after(b"--- "),
        name => name,
    };
    let mut patch = Vec::new();
    for &line in header {
        if DROPPED_HEADER_LINES
            .iter()
            .any(|prefix| line.starts_with(prefix))
            || (keep_file
                && [&b"new file mode"[..], b"deleted file mode", b"index "]
                    .iter()
                    .any(|prefix| line.starts_with(prefix)))
        {
            continue;
        }
        let line = if renamed && line.starts_with(b"diff --git ") {
            [
                &b"diff --git "[..],
                &side_name(name, b'a'),
                b" ",
                &side_name(name, b'b'),
            ]
            .concat()
        } else if let Some(old) = line.strip_prefix(b"--- ")
            && (renamed || (keep_file && old == NO_FILE))
        {
            [&b"--- "[..], &side_name(name, b'a')].concat()
        } else if let Some(new) = line.strip_prefix(b"+++ ")
            && keep_file
            && new == NO_FILE
        {
            [&b"+++ "[..], &side_name(name, b'b')].concat()
        } else {
            line.to_vec()
        };
        patch.extend(line);
        patch.push(b'\n');
    }
    patch
}

/// `name` as git prints it (`b/x`, or `"b/x"` when quoted) with its side
/// prefix made `side` (`a` or `b`).
fn side_name(name: &[u8], side: u8) -> Vec<u8> {
    let mut name = name.to_vec();
    let at = usize::from(name.first() == Some(&b'"'));
    if name.get(at + 1) == Some(&b'/') {
        name[at] = side;
    }
    name
}
