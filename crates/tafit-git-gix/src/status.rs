//! The change list: HEAD's tree against the index (staged), the index against
//! the work tree (unstaged), and untracked files, read in parallel by gix.

use gix::bstr::BStr;
use gix::diff::index::ChangeRef;
use gix::dir::entry::Kind;
use gix::status::index_worktree::Item as WorktreeItem;
use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
use tafit_core::{CancellationToken, ChangeKind, FileChange, GitError, RepositoryStatus};

use crate::backend_error;

pub(crate) fn status(
    repo: &gix::Repository,
    cancel: &CancellationToken,
) -> Result<RepositoryStatus, GitError> {
    cancel.check()?;
    let mut status = RepositoryStatus::default();
    if repo.workdir().is_none() {
        return Ok(status);
    }
    let items = repo
        .status(gix::progress::Discard)
        .map_err(backend_error)?
        .should_interrupt_owned(cancel.flag())
        .into_iter(None)
        .map_err(backend_error)?;
    for item in items {
        // An interrupted walk ends early and silently; don't pass it off as complete.
        cancel.check()?;
        match item.map_err(backend_error)? {
            gix::status::Item::TreeIndex(change) => status.staged.push(staged(&change)),
            gix::status::Item::IndexWorktree(item) => match item {
                WorktreeItem::Modification {
                    rela_path,
                    status: entry,
                    ..
                } => {
                    if let Some(kind) = unstaged_kind(&entry) {
                        status.unstaged.push(file(rela_path.as_ref(), kind));
                    }
                }
                WorktreeItem::DirectoryContents { entry, .. } => {
                    if entry.status == gix::dir::entry::Status::Untracked {
                        status.untracked.push(untracked(&entry));
                    }
                }
                // Only with work-tree rename tracking, which is off as in `git status`.
                WorktreeItem::Rewrite { .. } => {}
            },
        }
    }
    cancel.check()?;
    for group in [
        &mut status.staged,
        &mut status.unstaged,
        &mut status.untracked,
    ] {
        group.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    }
    Ok(status)
}

fn staged(change: &ChangeRef<'_, '_>) -> FileChange {
    match change {
        ChangeRef::Addition { location, .. } => file(location, ChangeKind::Added),
        ChangeRef::Deletion { location, .. } => file(location, ChangeKind::Deleted),
        ChangeRef::Modification { location, .. } => file(location, ChangeKind::Modified),
        ChangeRef::Rewrite {
            location,
            copy: true,
            ..
        } => file(location, ChangeKind::Added),
        ChangeRef::Rewrite {
            source_location,
            location,
            ..
        } => file(
            location,
            ChangeKind::Renamed {
                from: source_location.to_string(),
            },
        ),
    }
}

fn unstaged_kind<T, U>(entry: &EntryStatus<T, U>) -> Option<ChangeKind> {
    match entry {
        EntryStatus::Change(Change::Removed) => Some(ChangeKind::Deleted),
        EntryStatus::Change(_) | EntryStatus::Conflict { .. } => Some(ChangeKind::Modified),
        // `git add -N`: git lists it as a new file not yet staged.
        EntryStatus::IntentToAdd => Some(ChangeKind::Added),
        // Unchanged: only the stat data was stale.
        EntryStatus::NeedsUpdate(_) => None,
    }
}

fn untracked(entry: &gix::dir::Entry) -> FileChange {
    let mut path = entry.rela_path.to_string();
    if matches!(entry.disk_kind, Some(Kind::Directory | Kind::Repository)) {
        path.push('/');
    }
    FileChange {
        path,
        kind: ChangeKind::Added,
    }
}

fn file(path: &BStr, kind: ChangeKind) -> FileChange {
    FileChange {
        path: path.to_string(),
        kind,
    }
}
