//! The parts of a line that changed, when it is shown beside the line it
//! replaced: Myers on tokens (words, runs of spaces, single symbols).

use std::ops::Range;

use crate::WhitespaceMode;
use crate::align::{Interner, Op, myers};

/// Tokens of one line, past its common prefix and suffix, compared token by
/// token; a longer line is only trimmed to its common ends.
const MAX_TOKENS: usize = 500;

/// Tokens removed plus added beyond which two lines are too different for
/// word-level highlights to help: the middle between their common ends is
/// highlighted whole instead.
const MAX_TOKEN_COST: usize = 16;

/// The byte ranges that changed in `old` and in `new`, a line edited into
/// the other. Adjacent changed tokens make one range. With
/// `WhitespaceMode::Ignore`, whitespace is left out of the comparison, so it
/// is never highlighted on its own.
pub(crate) fn changed_ranges(
    old: &str,
    new: &str,
    whitespace: WhitespaceMode,
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (old_tokens, new_tokens) = (tokens(old, whitespace), tokens(new, whitespace));
    let mut interner = Interner::new();
    let old_ids: Vec<u32> = old_tokens
        .iter()
        .map(|token| interner.id(&old[token.clone()]))
        .collect();
    let new_ids: Vec<u32> = new_tokens
        .iter()
        .map(|token| interner.id(&new[token.clone()]))
        .collect();

    let prefix = old_ids
        .iter()
        .zip(&new_ids)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_ids[prefix..]
        .iter()
        .rev()
        .zip(new_ids[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_middle = prefix..old_ids.len() - suffix;
    let new_middle = prefix..new_ids.len() - suffix;

    let mut ops = Vec::new();
    let compared = old_middle.len() <= MAX_TOKENS
        && new_middle.len() <= MAX_TOKENS
        && myers(
            &old_ids[old_middle.clone()],
            &new_ids[new_middle.clone()],
            MAX_TOKEN_COST,
            &mut ops,
        );
    if !compared {
        let span = |tokens: &[Range<usize>], middle: Range<usize>| -> Vec<Range<usize>> {
            match (tokens[middle.clone()].first(), tokens[middle].last()) {
                (Some(first), Some(last)) => std::iter::once(first.start..last.end).collect(),
                _ => Vec::new(),
            }
        };
        return (span(&old_tokens, old_middle), span(&new_tokens, new_middle));
    }

    let (mut old_ranges, mut new_ranges) = (Vec::new(), Vec::new());
    let (mut o, mut n) = (old_middle.start, new_middle.start);
    for op in ops {
        match op {
            Op::Equal => {
                o += 1;
                n += 1;
            }
            Op::Delete => {
                push_merged(&mut old_ranges, old_tokens[o].clone());
                o += 1;
            }
            Op::Insert => {
                push_merged(&mut new_ranges, new_tokens[n].clone());
                n += 1;
            }
        }
    }
    (old_ranges, new_ranges)
}

/// Appends `range`, joining it onto the last one when they touch.
fn push_merged(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    match ranges.last_mut() {
        Some(last) if last.end == range.start => last.end = range.end,
        _ => ranges.push(range),
    }
}

/// Splits `line` into words (letters, digits, `_`), runs of whitespace, and
/// single other characters, as byte ranges. With `WhitespaceMode::Ignore`,
/// the runs of whitespace are left out.
fn tokens(line: &str, whitespace: WhitespaceMode) -> Vec<Range<usize>> {
    #[derive(PartialEq)]
    enum Class {
        Word,
        Space,
        Symbol,
    }
    let class = |c: char| {
        if c.is_alphanumeric() || c == '_' {
            Class::Word
        } else if c.is_whitespace() {
            Class::Space
        } else {
            Class::Symbol
        }
    };
    let mut tokens: Vec<Range<usize>> = Vec::new();
    let mut previous = None;
    for (at, c) in line.char_indices() {
        let current = class(c);
        let joins = current != Class::Symbol && previous.as_ref() == Some(&current);
        match tokens.last_mut() {
            Some(last) if joins => last.end = at + c.len_utf8(),
            _ => tokens.push(at..at + c.len_utf8()),
        }
        previous = Some(current);
    }
    if whitespace == WhitespaceMode::Ignore {
        tokens.retain(|token| !line[token.clone()].trim().is_empty());
    }
    tokens
}
