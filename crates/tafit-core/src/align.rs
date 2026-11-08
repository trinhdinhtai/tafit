//! Aligning two sequences (a line's tokens) into an edit script with Myers,
//! with a limit so a pathological input can't stall a read.

use std::collections::HashMap;
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
