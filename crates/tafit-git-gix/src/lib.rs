//! `GitBackend` implemented with gix (reads only; writes go through the git CLI).

pub mod cli;
mod detect;
mod status;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tafit_core::{
    CancellationToken, GitBackend, GitEnvironment, GitError, GitRepository, Head, RepositoryInfo,
    RepositoryStatus,
};

use crate::cli::GitCli;

#[derive(Clone, Debug, Default)]
pub struct GixBackend {
    /// The git executable to run; `None` searches `PATH`.
    git: Option<PathBuf>,
}

impl GixBackend {
    /// Runs the git at `program` instead of the one on `PATH`.
    pub fn with_git(program: impl Into<PathBuf>) -> Self {
        Self {
            git: Some(program.into()),
        }
    }

    fn git(&self) -> Result<GitCli, GitError> {
        match &self.git {
            Some(program) => Ok(GitCli::new(program.clone())),
            None => GitCli::find(),
        }
    }
}

impl GitBackend for GixBackend {
    fn detect_git(&self, cancel: &CancellationToken) -> Result<GitEnvironment, GitError> {
        detect::detect_git(&self.git()?, cancel)
    }

    fn open(
        &self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<Arc<dyn GitRepository>, GitError> {
        cancel.check()?;
        let repo = gix::open(path).map_err(|err| {
            // gix classifies "not a git repository" (and a missing path) as not-found.
            if err.is_not_found() {
                if path.exists() {
                    GitError::NotARepository(path.to_owned())
                } else {
                    GitError::PathNotFound(path.to_owned())
                }
            } else {
                backend_error(err)
            }
        })?;
        Ok(Arc::new(GixRepository {
            repo: repo.into_sync(),
        }))
    }
}

struct GixRepository {
    repo: gix::ThreadSafeRepository,
}

impl GitRepository for GixRepository {
    fn info(&self, cancel: &CancellationToken) -> Result<RepositoryInfo, GitError> {
        cancel.check()?;
        let repo = self.repo.to_thread_local();
        let head = match repo.head().map_err(backend_error)?.kind {
            gix::head::Kind::Symbolic(reference) => {
                Head::Branch(reference.name.shorten().to_string())
            }
            gix::head::Kind::Unborn(name) => Head::Unborn(name.shorten().to_string()),
            gix::head::Kind::Detached { target, .. } => {
                Head::Detached(target.to_hex_with_len(7).to_string())
            }
        };
        Ok(RepositoryInfo {
            name: repository_name(&repo),
            head,
        })
    }

    fn status(&self, cancel: &CancellationToken) -> Result<RepositoryStatus, GitError> {
        status::status(&self.repo.to_thread_local(), cancel)
    }
}

fn repository_name(repo: &gix::Repository) -> String {
    let dir = repo.workdir().unwrap_or_else(|| repo.git_dir());
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| dir.to_string_lossy());
    // A bare repo's folder is conventionally `name.git`.
    if repo.workdir().is_none() {
        name.strip_suffix(".git").unwrap_or(&name).to_owned()
    } else {
        name.into_owned()
    }
}

fn backend_error(err: impl std::fmt::Display) -> GitError {
    GitError::Backend(err.to_string())
}
