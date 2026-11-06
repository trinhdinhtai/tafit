//! Domain types and the Git backend interface. Never depends on GPUI.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What HEAD points at in a Repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    /// HEAD is a branch with at least one commit.
    Branch(String),
    /// HEAD is a branch that has no commit yet (a freshly initialized repo).
    Unborn(String),
    /// HEAD points straight at a commit; holds its abbreviated id.
    Detached(String),
}

/// What tafit shows about a Repository right after opening it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepositoryInfo {
    /// Display name: the work tree's folder name, or the git dir's for a bare repo.
    pub name: String,
    pub head: Head,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("path does not exist: {}", .0.display())]
    PathNotFound(PathBuf),
    #[error("not a Git repository: {}", .0.display())]
    NotARepository(PathBuf),
    #[error("operation cancelled")]
    Cancelled,
    #[error("{0}")]
    Backend(String),
}

/// Cooperative cancellation for long-running reads; cloning shares the flag.
#[derive(Clone, Debug, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// `Err(GitError::Cancelled)` once cancelled, for use with `?`.
    pub fn check(&self) -> Result<(), GitError> {
        if self.is_cancelled() {
            Err(GitError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Opens Repositories. Implemented by `tafit-git-gix`.
pub trait GitBackend: Send + Sync {
    /// Opens the Repository at exactly `path` (no search through parent directories).
    fn open(
        &self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<Arc<dyn GitRepository>, GitError>;
}

/// An open Repository. Read methods take a `CancellationToken`.
pub trait GitRepository: Send + Sync {
    fn info(&self, cancel: &CancellationToken) -> Result<RepositoryInfo, GitError>;
}
