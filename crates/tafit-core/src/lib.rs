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

/// How one file changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    /// Content, mode or type changed; also a conflicted file.
    Modified,
    Deleted,
    /// Moved from `from` (a repo-relative path) to the change's `path`.
    Renamed {
        from: String,
    },
}

/// One file in the change list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    /// Repo-relative, with `/` separators. An untracked directory shown as a
    /// whole ends in `/`.
    pub path: String,
    pub kind: ChangeKind,
}

/// A Repository's changes, each group sorted by path. A file staged in part
/// is in both `staged` and `unstaged`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryStatus {
    /// HEAD's tree against the index.
    pub staged: Vec<FileChange>,
    /// The index against the work tree.
    pub unstaged: Vec<FileChange>,
    /// Files git doesn't track and doesn't ignore; always `ChangeKind::Added`.
    pub untracked: Vec<FileChange>,
    /// A merge stopped before committing (`MERGE_HEAD` exists), so a commit
    /// concludes it even with nothing staged.
    pub merge_in_progress: bool,
}

impl RepositoryStatus {
    /// No changed files. A merge in progress can still be committed.
    pub fn is_clean(&self) -> bool {
        self.staged.is_empty() && self.unstaged.is_empty() && self.untracked.is_empty()
    }
}

/// A git version, as printed by `git --version`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitVersion {
    /// The version as git prints it, e.g. `2.53.0.windows.2`.
    pub text: String,
    pub major: u32,
    pub minor: u32,
}

impl GitVersion {
    /// The oldest git tafit is built against (major, minor). Older versions
    /// still run, but the user is told to update.
    pub const MINIMUM: (u32, u32) = (2, 53);

    /// Parses `git --version` output such as `git version 2.53.0.windows.2` or
    /// `git version 2.39.3 (Apple Git-146)`: the first word that starts with
    /// `<major>.<minor>`.
    pub fn parse(output: &str) -> Option<Self> {
        output.split_whitespace().find_map(|word| {
            let mut parts = word.split('.');
            let major = leading_number(parts.next()?)?;
            let minor = leading_number(parts.next()?)?;
            Some(Self {
                text: word.to_owned(),
                major,
                minor,
            })
        })
    }

    pub fn is_supported(&self) -> bool {
        (self.major, self.minor) >= Self::MINIMUM
    }

    /// `MINIMUM` as users read it, e.g. `2.53`.
    pub fn minimum_text() -> String {
        let (major, minor) = Self::MINIMUM;
        format!("{major}.{minor}")
    }
}

fn leading_number(part: &str) -> Option<u32> {
    let end = part
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(part.len());
    part[..end].parse().ok()
}

/// The git found on this machine and the signing tools it will call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitEnvironment {
    pub git_path: PathBuf,
    pub version: GitVersion,
    /// The program git runs to sign with OpenPGP (`gpg.program`, else `gpg`);
    /// `None` when it can't be found.
    pub gpg_path: Option<PathBuf>,
    /// The program git runs to sign with SSH (`gpg.ssh.program`, else
    /// `ssh-keygen`); `None` when it can't be found.
    pub ssh_keygen_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GitError {
    #[error("path does not exist: {}", .0.display())]
    PathNotFound(PathBuf),
    #[error("not a Git repository: {}", .0.display())]
    NotARepository(PathBuf),
    #[error("operation cancelled")]
    Cancelled,
    /// No git executable could be started.
    #[error("git was not found: {0}")]
    GitNotFound(String),
    /// A git command ran and exited unsuccessfully; holds everything it printed.
    #[error("{}", command_failed_message(.command, *.exit_code, .stdout, .stderr))]
    CommandFailed {
        /// The command line, e.g. `git --version`.
        command: String,
        /// `None` when the process was ended by a signal.
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    #[error("{0}")]
    Backend(String),
}

fn command_failed_message(
    command: &str,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> String {
    let mut message = match exit_code {
        Some(code) => format!("`{command}` failed with exit code {code}"),
        None => format!("`{command}` was terminated"),
    };
    for output in [stderr.trim(), stdout.trim()] {
        if !output.is_empty() {
            message.push_str("\n\n");
            message.push_str(output);
        }
    }
    message
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

    /// The underlying flag, for libraries that poll an `AtomicBool` themselves.
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
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

/// Opens Repositories and runs the system git. Implemented by `tafit-git-gix`.
pub trait GitBackend: Send + Sync {
    /// Finds the system git, its version, and the signing tools it calls.
    fn detect_git(&self, cancel: &CancellationToken) -> Result<GitEnvironment, GitError>;

    /// Opens the Repository at exactly `path` (no search through parent directories).
    fn open(
        &self,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<Arc<dyn GitRepository>, GitError>;
}

/// An open Repository. Every method takes a `CancellationToken`; writes run
/// the system git (ADR-0002), so the user's hooks and config apply.
pub trait GitRepository: Send + Sync {
    fn info(&self, cancel: &CancellationToken) -> Result<RepositoryInfo, GitError>;

    /// The change list: staged, unstaged and untracked files. Empty for a
    /// bare Repository, which has no work tree.
    fn status(&self, cancel: &CancellationToken) -> Result<RepositoryStatus, GitError>;

    /// Stages each repo-relative path (a file, or an untracked folder ending
    /// in `/`) as it is in the work tree, a deletion included.
    fn stage(&self, paths: &[String], cancel: &CancellationToken) -> Result<(), GitError>;

    /// Resets each repo-relative path in the index to HEAD; on a branch with
    /// no commit yet, removes it from the index. The work tree is untouched.
    fn unstage(&self, paths: &[String], cancel: &CancellationToken) -> Result<(), GitError>;

    /// Commits what is staged with `message`, as `git commit` does: hooks
    /// run, and `commit.gpgsign` signs it.
    fn commit(&self, message: &str, cancel: &CancellationToken) -> Result<(), GitError>;
}
