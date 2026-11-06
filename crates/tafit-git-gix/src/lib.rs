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
            workdir: repo.workdir().map(Path::to_owned),
            repo: repo.into_sync(),
            backend: self.clone(),
        }))
    }
}

struct GixRepository {
    repo: gix::ThreadSafeRepository,
    /// `None` for a bare Repository.
    workdir: Option<PathBuf>,
    /// Runs the writes.
    backend: GixBackend,
}

impl GixRepository {
    /// Runs git in the work tree with `input` on stdin.
    fn run_git(
        &self,
        args: &[&str],
        input: impl Into<Vec<u8>>,
        cancel: &CancellationToken,
    ) -> Result<(), GitError> {
        let workdir = self.workdir.as_deref().ok_or_else(|| {
            GitError::Backend("a bare Repository has no work tree to change".to_owned())
        })?;
        self.backend
            .git()?
            .run_with_input(Some(workdir), args, input, cancel)
            .map(drop)
    }

    /// Runs a command that reads its paths from stdin, NUL-separated, and
    /// matches them literally, so a file named `*.txt` is never a glob. No
    /// paths does nothing: an empty pathspec would mean the whole tree.
    ///
    /// Only for commands that run no hooks: git passes
    /// `--literal-pathspecs` on to them as `GIT_LITERAL_PATHSPECS`.
    fn run_on_paths(
        &self,
        args: &[&str],
        paths: &[String],
        cancel: &CancellationToken,
    ) -> Result<(), GitError> {
        if paths.is_empty() {
            return Ok(());
        }
        let args: Vec<&str> = ["--literal-pathspecs"]
            .into_iter()
            .chain(args.iter().copied())
            .chain(["--pathspec-from-file=-", "--pathspec-file-nul"])
            .collect();
        self.run_git(&args, nul_separated(paths), cancel)
    }
}

fn nul_separated(paths: &[String]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(paths.iter().map(|path| path.len() + 1).sum());
    for path in paths {
        bytes.extend_from_slice(path.as_bytes());
        bytes.push(0);
    }
    bytes
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

    fn stage(&self, paths: &[String], cancel: &CancellationToken) -> Result<(), GitError> {
        // `--all` stages deletions too.
        self.run_on_paths(&["add", "--all"], paths, cancel)
    }

    fn unstage(&self, paths: &[String], cancel: &CancellationToken) -> Result<(), GitError> {
        // Unlike `restore --staged`, `reset` also works before the first commit.
        self.run_on_paths(&["reset", "--quiet"], paths, cancel)
    }

    fn commit(&self, message: &str, cancel: &CancellationToken) -> Result<(), GitError> {
        self.run_git(&["commit", "--quiet", "--file=-"], message, cancel)
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
