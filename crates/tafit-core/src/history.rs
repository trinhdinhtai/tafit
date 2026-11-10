//! The commit history: an index of every commit and its parents, built once
//! for the whole history, and what is shown about each commit, read a few
//! hundred at a time as they come on screen.

use std::collections::HashMap;
use std::fmt;

/// A commit's SHA-1 id.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitId(pub [u8; 20]);

impl CommitId {
    /// The first `len` hex digits, as `git log --abbrev` shows them.
    pub fn short(&self, len: usize) -> String {
        let mut hex = self.to_string();
        hex.truncate(len);
        hex
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CommitId({self})")
    }
}

/// Every commit of the history in the order it is shown, newest first, as
/// `git rev-list --date-order` lists them: no commit before its children.
/// Each commit's row is its index; parents are named by row too.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HistoryIndex {
    ids: Vec<CommitId>,
    /// Row `r`'s parents are `parents[parent_starts[r]..parent_starts[r + 1]]`.
    parent_starts: Vec<u32>,
    parents: Vec<u32>,
}

impl HistoryIndex {
    /// Builds the index from commits in history order, each with its
    /// parents' ids. A parent that isn't in the history (beyond a shallow
    /// clone's edge) is left out.
    pub fn new(commits: Vec<(CommitId, Vec<CommitId>)>) -> Self {
        let (ids, parents): (Vec<CommitId>, Vec<Vec<CommitId>>) = commits.into_iter().unzip();
        let order = (0..row_u32(ids.len())).collect();
        let parents = parent_rows(&ids, &parents);
        Self::from_order(ids, &parents, order)
    }

    /// Orders `commits` as `git rev-list --date-order` does and builds the
    /// index: no commit before all of its children, and otherwise newest
    /// commit time first, ties in the order the commits became ready. Each
    /// commit comes with its parents and commit time, in any order; the
    /// ones no other commit names as a parent start the history, in the
    /// order given.
    pub fn in_date_order(commits: Vec<(CommitId, Vec<CommitId>, i64)>) -> Self {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        let mut ids = Vec::with_capacity(commits.len());
        let mut parents = Vec::with_capacity(commits.len());
        let mut times = Vec::with_capacity(commits.len());
        for (id, commit_parents, time) in commits {
            ids.push(id);
            parents.push(commit_parents);
            times.push(time);
        }
        let parents = parent_rows(&ids, &parents);
        // How many of each commit's children are still to be shown.
        let mut children = vec![0u32; ids.len()];
        for &parent in parents.iter().flatten() {
            children[parent as usize] += 1;
        }
        // Newest first; among equal times, the one ready first.
        let mut ready = BinaryHeap::new();
        let mut readied = 0u64;
        for (ix, &count) in children.iter().enumerate() {
            if count == 0 {
                ready.push((times[ix], Reverse(readied), ix));
                readied += 1;
            }
        }
        let mut order = Vec::with_capacity(ids.len());
        while let Some((_, _, ix)) = ready.pop() {
            order.push(row_u32(ix));
            for &parent in &parents[ix] {
                let parent = parent as usize;
                children[parent] -= 1;
                if children[parent] == 0 {
                    ready.push((times[parent], Reverse(readied), parent));
                    readied += 1;
                }
            }
        }
        Self::from_order(ids, &parents, order)
    }

    /// The index of `ids` shown in `order`, where `parents[i]` holds the
    /// positions in `ids` of `ids[i]`'s parents.
    fn from_order(ids: Vec<CommitId>, parents: &[Vec<u32>], order: Vec<u32>) -> Self {
        let mut row_of = vec![0u32; ids.len()];
        for (row, &ix) in order.iter().enumerate() {
            row_of[ix as usize] = row_u32(row);
        }
        let mut index = Self {
            ids: Vec::with_capacity(order.len()),
            parent_starts: Vec::with_capacity(order.len() + 1),
            parents: Vec::with_capacity(order.len()),
        };
        for &ix in &order {
            index.ids.push(ids[ix as usize]);
            index.parent_starts.push(row_u32(index.parents.len()));
            index.parents.extend(
                parents[ix as usize]
                    .iter()
                    .map(|&parent| row_of[parent as usize]),
            );
        }
        index.parent_starts.push(row_u32(index.parents.len()));
        index
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// The commit at `row`.
    pub fn id(&self, row: usize) -> Option<CommitId> {
        self.ids.get(row).copied()
    }

    /// The commits at `rows`, clamped to the history.
    pub fn ids(&self, rows: std::ops::Range<usize>) -> &[CommitId] {
        let end = rows.end.min(self.ids.len());
        &self.ids[rows.start.min(end)..end]
    }

    /// The rows of the parents of the commit at `row`, first parent first.
    pub fn parents(&self, row: usize) -> &[u32] {
        match (self.parent_starts.get(row), self.parent_starts.get(row + 1)) {
            (Some(&start), Some(&end)) => &self.parents[start as usize..end as usize],
            _ => &[],
        }
    }
}

fn row_u32(row: usize) -> u32 {
    u32::try_from(row).expect("a history has fewer than 4 billion commits")
}

/// What kind of ref a label names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RefKind {
    /// A detached HEAD; on a branch, HEAD is shown on the branch's label.
    Head,
    Branch,
    RemoteBranch,
    Tag,
}

/// A ref shown on the commit it points at (a tag's, once peeled).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RefLabel {
    pub kind: RefKind,
    /// Without its namespace: `main`, `origin/main`, `v1`, or `HEAD`.
    pub name: String,
    /// HEAD is this label: the detached HEAD, or the branch checked out.
    pub head: bool,
}

/// The labels of every commit a shown ref points at. Each commit's labels
/// are in the order they are shown: HEAD first, then branches, remote
/// branches and tags, each by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefLabels(HashMap<CommitId, Vec<RefLabel>>);

impl RefLabels {
    pub fn new(labels: impl IntoIterator<Item = (CommitId, RefLabel)>) -> Self {
        let mut by_commit: HashMap<CommitId, Vec<RefLabel>> = HashMap::new();
        for (id, label) in labels {
            by_commit.entry(id).or_default().push(label);
        }
        for labels in by_commit.values_mut() {
            labels.sort_unstable_by(|a, b| {
                (!a.head, a.kind, &a.name).cmp(&(!b.head, b.kind, &b.name))
            });
        }
        Self(by_commit)
    }

    /// `id`'s labels; empty for a commit no shown ref points at.
    pub fn of(&self, id: CommitId) -> &[RefLabel] {
        self.0.get(&id).map_or(&[], Vec::as_slice)
    }

    /// The commits the labels are on, in no order: where the history starts.
    pub fn commits(&self) -> impl Iterator<Item = CommitId> + '_ {
        self.0.keys().copied()
    }
}

/// A Repository's history as one read gives it: the index, and the refs it
/// was walked from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    pub index: HistoryIndex,
    pub refs: RefLabels,
}

/// When a commit was authored, as git records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitTime {
    pub seconds: i64,
    /// The author's UTC offset, in seconds east.
    pub offset: i32,
}

/// What the history list shows about one commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSummary {
    pub id: CommitId,
    /// The message's first paragraph on one line, as `git log --oneline`.
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub author_time: CommitTime,
}

/// Each commit's parents as positions in `ids`, leaving out a parent that
/// isn't among them (beyond a shallow clone's edge).
fn parent_rows(ids: &[CommitId], parents: &[Vec<CommitId>]) -> Vec<Vec<u32>> {
    let positions: HashMap<CommitId, u32, BuildIdHasher> = ids
        .iter()
        .enumerate()
        .map(|(ix, id)| (*id, row_u32(ix)))
        .collect();
    parents
        .iter()
        .map(|parents| {
            parents
                .iter()
                .filter_map(|parent| positions.get(parent).copied())
                .collect()
        })
        .collect()
}

/// Hashes a `CommitId` by its first eight bytes: a SHA-1 is already evenly
/// spread, and a general-purpose hash of all twenty bytes is several times
/// slower over millions of commits.
#[derive(Clone, Copy, Default)]
struct BuildIdHasher;

impl std::hash::BuildHasher for BuildIdHasher {
    type Hasher = IdHasher;

    fn build_hasher(&self) -> IdHasher {
        IdHasher(0)
    }
}

struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        // `[u8; 20]` hashes as its length, then its bytes.
        if let Some(first) = bytes.first_chunk::<8>() {
            self.0 = u64::from_ne_bytes(*first);
        }
    }

    fn write_usize(&mut self, _: usize) {}
}
