//! Aligning two sequences (a file's lines, or a line's tokens) into an edit
//! script: patience anchors on items unique to both sides, Myers between
//! them, each with a limit so a pathological input can't stall a read.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::Hash;

/// One step of an edit script, walking both sides from the start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Op {
    /// The next item of each side is the same.
    Equal,
    /// The next old item is gone.
    Delete,
    /// The next new item is new.
    Insert,
}

/// Gives each distinct key a small number, so the aligners compare `u32`s.
pub(crate) struct Interner<K> {
    ids: HashMap<K, u32>,
}

impl<K: Eq + Hash> Interner<K> {
    pub(crate) fn new() -> Self {
        Self {
            ids: HashMap::new(),
        }
    }

    pub(crate) fn id(&mut self, key: K) -> u32 {
        let next = self.ids.len() as u32;
        *self.ids.entry(key).or_insert(next)
    }
}

/// How deep patience recurses before the rest of a region goes to Myers.
const MAX_PATIENCE_DEPTH: u32 = 64;

/// The edit cost (lines deleted plus inserted) Myers searches up to; past
/// it, a region shows as all deleted then all inserted. Its memory grows
/// with the square of this.
const MAX_LINE_COST: usize = 1_024;

/// Aligns two files' lines: unchanged lines unique to both sides anchor the
/// alignment, so a moved block shows as moved rather than shifting the rest
/// of the file; the stretches between anchors are aligned by Myers.
pub(crate) fn align_lines(old: &[u32], new: &[u32]) -> Vec<Op> {
    let mut ops = Vec::with_capacity(old.len().max(new.len()));
    patience(old, new, 0, &mut ops);
    slide_down(&mut ops, old, new);
    ops
}

/// Moves each run of only deleted or only inserted lines as far down as it
/// can go without changing what it removes or adds, as git compacts its
/// groups: of `}` `` `fn a() {` … `}` inserted after a `}`, the block's own
/// `}` closes it rather than the `}` before it.
fn slide_down(ops: &mut [Op], old: &[u32], new: &[u32]) {
    let (mut start, mut i, mut j) = (0, 0, 0);
    // The run being slid: its first op, and where it starts on its side.
    let mut run: Option<(usize, usize)> = None;
    while start < ops.len() {
        match ops[start] {
            Op::Equal => {
                if let Some((first, from)) = run {
                    let kind = ops[first];
                    let side = if kind == Op::Delete { old } else { new };
                    let at = if kind == Op::Delete { i } else { j };
                    if side[from] == side[at] {
                        // The run's first line equals the equal line after it:
                        // swap them, and the run starts one line further down.
                        ops.swap(first, start);
                        run = Some((first + 1, from + 1));
                    } else {
                        run = None;
                    }
                }
                i += 1;
                j += 1;
            }
            kind => {
                let at = if kind == Op::Delete { i } else { j };
                run = match run {
                    Some((first, from)) if ops[first] == kind => Some((first, from)),
                    // A mixed run is an edit, left where it is.
                    Some(_) => None,
                    None if start == 0 || ops[start - 1] == Op::Equal => Some((start, at)),
                    None => None,
                };
                if kind == Op::Delete {
                    i += 1;
                } else {
                    j += 1;
                }
            }
        }
        start += 1;
    }
}

fn patience(old: &[u32], new: &[u32], depth: u32, ops: &mut Vec<Op>) {
    let (old, new, suffix) = strip_common(old, new, ops);
    if old.is_empty() || new.is_empty() {
        ops.extend(std::iter::repeat_n(Op::Delete, old.len()));
        ops.extend(std::iter::repeat_n(Op::Insert, new.len()));
    } else {
        let anchors = if depth < MAX_PATIENCE_DEPTH {
            unique_anchors(old, new)
        } else {
            Vec::new()
        };
        if anchors.is_empty() {
            myers_or_replace(old, new, MAX_LINE_COST, ops);
        } else {
            let (mut o, mut n) = (0, 0);
            for (anchor_o, anchor_n) in anchors {
                patience(&old[o..anchor_o], &new[n..anchor_n], depth + 1, ops);
                ops.push(Op::Equal);
                (o, n) = (anchor_o + 1, anchor_n + 1);
            }
            patience(&old[o..], &new[n..], depth + 1, ops);
        }
    }
    ops.extend(std::iter::repeat_n(Op::Equal, suffix));
}

/// Pushes the common prefix as equal, and returns what is left of each side
/// plus the length of the common suffix the caller pushes last.
fn strip_common<'a>(
    old: &'a [u32],
    new: &'a [u32],
    ops: &mut Vec<Op>,
) -> (&'a [u32], &'a [u32], usize) {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    ops.extend(std::iter::repeat_n(Op::Equal, prefix));
    let (old, new) = (&old[prefix..], &new[prefix..]);
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (
        &old[..old.len() - suffix],
        &new[..new.len() - suffix],
        suffix,
    )
}

/// Pairs of positions `(old, new)` of items that occur exactly once on each
/// side, the longest run of them that is in order on both sides.
fn unique_anchors(old: &[u32], new: &[u32]) -> Vec<(usize, usize)> {
    // Per item: where it is on each side, or `None` once seen twice there.
    let mut seen: HashMap<u32, (Option<usize>, Option<usize>)> = HashMap::new();
    for (ix, &item) in old.iter().enumerate() {
        match seen.entry(item) {
            Entry::Vacant(entry) => {
                entry.insert((Some(ix), None));
            }
            Entry::Occupied(mut entry) => entry.get_mut().0 = None,
        }
    }
    // Old positions of items unique on both sides, in new order. An item
    // seen twice on the new side is marked by a sentinel.
    const TWICE: usize = usize::MAX;
    for (ix, &item) in new.iter().enumerate() {
        if let Some((Some(_), at_new)) = seen.get_mut(&item) {
            *at_new = Some(if at_new.is_some() { TWICE } else { ix });
        }
    }
    let mut pairs: Vec<(usize, usize)> = seen
        .into_values()
        .filter_map(|sides| match sides {
            (Some(o), Some(n)) if n != TWICE => Some((o, n)),
            _ => None,
        })
        .collect();
    pairs.sort_unstable_by_key(|&(_, n)| n);
    longest_increasing_by_old(&pairs)
}

/// The longest subsequence of `pairs` (sorted by new position) whose old
/// positions increase too: patience sorting, O(n log n).
fn longest_increasing_by_old(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // `tails[k]`: the pair ending the best run of length k + 1 found so far.
    let mut tails: Vec<usize> = Vec::new();
    let mut previous: Vec<Option<usize>> = vec![None; pairs.len()];
    for (ix, &(old, _)) in pairs.iter().enumerate() {
        let k = tails.partition_point(|&tail| pairs[tail].0 < old);
        previous[ix] = k.checked_sub(1).map(|k| tails[k]);
        if k == tails.len() {
            tails.push(ix);
        } else {
            tails[k] = ix;
        }
    }
    let mut run = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(ix) = at {
        run.push(pairs[ix]);
        at = previous[ix];
    }
    run.reverse();
    run
}

/// Myers' shortest edit script, or when it costs more than `max_cost`, every
/// old item deleted then every new one inserted.
pub(crate) fn myers_or_replace(old: &[u32], new: &[u32], max_cost: usize, ops: &mut Vec<Op>) {
    if !myers(old, new, max_cost, ops) {
        ops.extend(std::iter::repeat_n(Op::Delete, old.len()));
        ops.extend(std::iter::repeat_n(Op::Insert, new.len()));
    }
}

/// Myers' O(ND) shortest edit script, pushed onto `ops`. Returns `false`,
/// pushing nothing, when it would cost more than `max_cost`.
pub(crate) fn myers(old: &[u32], new: &[u32], max_cost: usize, ops: &mut Vec<Op>) -> bool {
    let (n, m) = (old.len() as isize, new.len() as isize);
    let max = (old.len() + new.len()).min(max_cost) as isize;
    // `v[k + offset]`: the furthest x reached on diagonal k = x - y.
    let offset = max + 1;
    let mut v = vec![0isize; 2 * offset as usize + 1];
    // The `v` of each cost, its diagonals -d..=d only, for walking back.
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    'search: for d in 0..=max {
        for k in (-d..=d).step_by(2) {
            let down =
                k == -d || (k != d && v[(k - 1 + offset) as usize] < v[(k + 1 + offset) as usize]);
            let mut x = if down {
                v[(k + 1 + offset) as usize]
            } else {
                v[(k - 1 + offset) as usize] + 1
            };
            let mut y = x - k;
            while x < n && y < m && old[x as usize] == new[y as usize] {
                x += 1;
                y += 1;
            }
            v[(k + offset) as usize] = x;
            if x >= n && y >= m {
                trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
                found = Some(d);
                break 'search;
            }
        }
        trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
    }
    let Some(cost) = found else {
        return false;
    };
    // Walk back from (n, m), collecting the script in reverse.
    let mut script = Vec::with_capacity((n + m) as usize);
    let (mut x, mut y) = (n, m);
    for d in (0..=cost).rev() {
        let k = x - y;
        // `trace[d - 1]` holds diagonals -(d-1)..=(d-1).
        let at = |d: isize, k: isize| trace[d as usize][(k + d) as usize];
        let (prev_k, prev_x) = if d == 0 {
            (0, 0)
        } else {
            let down = k == -d || (k != d && at(d - 1, k - 1) < at(d - 1, k + 1));
            let prev_k = if down { k + 1 } else { k - 1 };
            (prev_k, at(d - 1, prev_k))
        };
        let prev_y = prev_x - prev_k;
        // The snake: equal items after the step.
        let (start_x, start_y) = if d == 0 {
            (0, 0)
        } else if prev_k == k + 1 {
            (prev_x, prev_y + 1)
        } else {
            (prev_x + 1, prev_y)
        };
        while x > start_x && y > start_y {
            script.push(Op::Equal);
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            script.push(if prev_k == k + 1 {
                Op::Insert
            } else {
                Op::Delete
            });
            (x, y) = (prev_x, prev_y);
        }
    }
    ops.extend(script.into_iter().rev());
    true
}
