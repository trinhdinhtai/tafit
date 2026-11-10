//! The history: the shown refs, the commits they reach in date order, and
//! what is shown about a commit.

use gix::bstr::ByteSlice;
use gix::revision::walk::Sorting;
use tafit_core::{
    CancellationToken, CommitId, CommitSummary, CommitTime, GitError, History, HistoryIndex,
    RefKind, RefLabel, RefLabels,
};

use crate::backend_error;

/// How many commits are walked between checks for cancelling.
const CANCEL_EVERY: usize = 4096;

pub(crate) fn history(
    repo: &gix::Repository,
    cancel: &CancellationToken,
) -> Result<History, GitError> {
    cancel.check()?;
    let refs = RefLabels::new(labels(repo)?);
    cancel.check()?;
    let tips: Vec<gix::ObjectId> = refs.commits().map(object_id).collect();
    // One newest-first pass reads each commit once; ordering it in memory
    // is several times faster than gix's topological walk, which reads
    // every commit again for each of its passes.
    let walk = repo
        .rev_walk(tips)
        .sorting(Sorting::ByCommitTime(Default::default()))
        .all()
        .map_err(backend_error)?;
    let mut commits = Vec::new();
    for (ix, info) in walk.enumerate() {
        if ix % CANCEL_EVERY == 0 {
            cancel.check()?;
        }
        let info = info.map_err(backend_error)?;
        let parents = info.parent_ids.iter().map(|id| commit_id(id)).collect();
        commits.push((commit_id(&info.id), parents, info.commit_time()));
    }
    cancel.check()?;
    Ok(History {
        index: HistoryIndex::in_date_order(commits),
        refs,
    })
}

/// HEAD, the branches, the remote branches and the tags, each on the
/// commit it points at. A remote's own HEAD, and a tag of something other
/// than a commit, aren't shown.
fn labels(repo: &gix::Repository) -> Result<Vec<(CommitId, RefLabel)>, GitError> {
    let mut labels = Vec::new();
    let head = repo.head().map_err(backend_error)?;
    let head_branch = head.referent_name().map(|name| name.as_bstr().to_owned());
    if head_branch.is_none()
        && let Some(id) = head.id()
    {
        labels.push((
            commit_id(&id),
            RefLabel {
                kind: RefKind::Head,
                name: "HEAD".to_owned(),
                head: true,
            },
        ));
    }
    let references = repo.references().map_err(backend_error)?;
    let groups = [
        (RefKind::Branch, references.local_branches()),
        (RefKind::RemoteBranch, references.remote_branches()),
        (RefKind::Tag, references.tags()),
    ];
    for (kind, iter) in groups {
        for reference in iter.map_err(backend_error)? {
            let mut reference = reference.map_err(backend_error)?;
            let name = reference.name().shorten().to_str_lossy().into_owned();
            if kind == RefKind::RemoteBranch && name.ends_with("/HEAD") {
                continue;
            }
            let head = head_branch.as_ref() == Some(&reference.name().as_bstr().to_owned());
            let id = match reference.peel_to_kind(gix::object::Kind::Commit) {
                Ok(commit) => commit_id(&commit.id),
                // A tag of a tree or blob names no commit.
                Err(_) if kind == RefKind::Tag => continue,
                Err(err) => return Err(backend_error(err)),
            };
            labels.push((id, RefLabel { kind, name, head }));
        }
    }
    Ok(labels)
}

pub(crate) fn commits(
    repo: &gix::Repository,
    ids: &[CommitId],
    cancel: &CancellationToken,
) -> Result<Vec<CommitSummary>, GitError> {
    ids.iter()
        .map(|&id| {
            cancel.check()?;
            let commit = repo.find_commit(object_id(id)).map_err(backend_error)?;
            let commit = commit.decode().map_err(backend_error)?;
            let author = commit.author().map_err(backend_error)?;
            let time = author.time().map_err(backend_error)?;
            Ok(CommitSummary {
                id,
                summary: commit.message().summary().to_str_lossy().into_owned(),
                author_name: author.name.to_str_lossy().into_owned(),
                author_email: author.email.to_str_lossy().into_owned(),
                author_time: CommitTime {
                    seconds: time.seconds,
                    offset: time.offset,
                },
            })
        })
        .collect()
}

fn commit_id(id: &gix::oid) -> CommitId {
    CommitId(
        id.as_bytes()
            .try_into()
            .expect("tafit reads SHA-1 Repositories only"),
    )
}

fn object_id(id: CommitId) -> gix::ObjectId {
    gix::ObjectId::from_bytes_or_panic(&id.0)
}
