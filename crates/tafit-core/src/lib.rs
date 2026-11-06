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

/// Which two versions of a file a diff compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSource {
    /// HEAD against the index.
    Staged,
    /// The index against the work tree.
    Unstaged,
    /// Nothing against an untracked file in the work tree.
    Untracked,
}

/// A Line Diff of one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDiff {
    /// The hunks, in file order; none when only the mode changed.
    Text(Vec<Hunk>),
    /// git found binary content; there is nothing to show line by line.
    Binary,
}

impl FileDiff {
    /// Parses `git diff` output for one file. Only the hunks are read, never
    /// the file names in the headers, so how git quotes a name can't matter.
    pub fn parse(output: &[u8]) -> Self {
        let mut hunks: Vec<Hunk> = Vec::new();
        // Lines still expected in the current hunk, old side and new side,
        // and the line numbers the next line gets.
        let (mut old_left, mut new_left) = (0u32, 0u32);
        let (mut old_next, mut new_next) = (0u32, 0u32);
        for raw in output.split(|&byte| byte == b'\n') {
            // `\ No newline at end of file` follows the line it is about, which
            // may be mid-hunk (`-a`, `\ …`, `+a`). No hunk line starts with `\`.
            if raw.starts_with(b"\\") {
                if let Some(line) = hunks.last_mut().and_then(|hunk| hunk.lines.last_mut()) {
                    line.no_newline_at_end = true;
                }
            } else if old_left > 0 || new_left > 0 {
                let hunk = hunks.last_mut().expect("counts are set by a hunk header");
                let (kind, text) = match raw.split_first() {
                    Some((b'+', text)) => (LineKind::Added, text),
                    Some((b'-', text)) => (LineKind::Removed, text),
                    Some((_, text)) => (LineKind::Context, text),
                    // An empty context line, should git ever drop its space.
                    None => (LineKind::Context, raw),
                };
                let (mut old_line, mut new_line) = (None, None);
                if kind != LineKind::Added {
                    old_line = Some(old_next);
                    old_next += 1;
                    old_left = old_left.saturating_sub(1);
                }
                if kind != LineKind::Removed {
                    new_line = Some(new_next);
                    new_next += 1;
                    new_left = new_left.saturating_sub(1);
                }
                hunk.lines.push(DiffLine {
                    kind,
                    old_line,
                    new_line,
                    // shortcut: a line that isn't UTF-8 is shown lossily, and
                    // a CRLF line loses its `\r`; staging lines (#11) will
                    // need the exact bytes.
                    text: String::from_utf8_lossy(text.strip_suffix(b"\r").unwrap_or(text))
                        .into_owned(),
                    no_newline_at_end: false,
                });
            } else if let Some(hunk) = Hunk::parse_header(raw) {
                (old_left, new_left) = (hunk.old_count, hunk.new_count);
                (old_next, new_next) = (hunk.old_start, hunk.new_start);
                hunks.push(hunk);
            } else if hunks.is_empty() && raw.starts_with(b"Binary files ") {
                return Self::Binary;
            }
        }
        Self::Text(hunks)
    }
}

/// One `@@` hunk of a Line Diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    /// The first line on the old side; 0 when that side is empty.
    pub old_start: u32,
    pub old_count: u32,
    pub new_start: u32,
    pub new_count: u32,
    /// The header line as git printed it, e.g. `@@ -1,3 +1,4 @@ fn main()`.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// `@@ -a[,b] +c[,d] @@ …`; a combined diff's `@@@` is not one.
    fn parse_header(raw: &[u8]) -> Option<Self> {
        let header = std::str::from_utf8(raw).ok()?.trim_end_matches('\r');
        let ranges = header.strip_prefix("@@ -")?;
        let (ranges, _) = ranges.split_once(" @@")?;
        let (old, new) = ranges.split_once(" +")?;
        let range = |range: &str| -> Option<(u32, u32)> {
            match range.split_once(',') {
                Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
                None => Some((range.parse().ok()?, 1)),
            }
        };
        let ((old_start, old_count), (new_start, new_count)) = (range(old)?, range(new)?);
        Some(Self {
            old_start,
            old_count,
            new_start,
            new_count,
            header: header.to_owned(),
            lines: Vec::new(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a hunk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: LineKind,
    /// 1-based; `None` for an added line.
    pub old_line: Option<u32>,
    /// 1-based; `None` for a removed line.
    pub new_line: Option<u32>,
    /// The line without its `+`/`-`/` ` marker and line ending.
    pub text: String,
    /// The file ends here without a newline.
    pub no_newline_at_end: bool,
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

    /// The Line Diff of one file from `source`, read from `git diff` with a
    /// fixed config so the user's (external diff, color, quotepath) can't
    /// change it (ADR-0002). `paths` is the file's repo-relative path, or a
    /// rename's old path then new path.
    fn diff(
        &self,
        source: DiffSource,
        paths: &[String],
        cancel: &CancellationToken,
    ) -> Result<FileDiff, GitError>;

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
