//! Runs the system git CLI, which handles every write and network operation
//! (ADR-0002). A command can be cancelled through its `CancellationToken`; that
//! ends git together with everything it started (hooks, ssh, credential helpers).

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tafit_core::{CancellationToken, GitError};

/// How often a running command checks its `CancellationToken`: starts short so
/// quick commands return fast, then backs off.
const POLL_START: Duration = Duration::from_micros(250);
const POLL_MAX: Duration = Duration::from_millis(5);

/// Everything a finished git command printed, and how it exited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitOutput {
    /// `None` when the process was ended by a signal.
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }
}

/// One git executable.
#[derive(Clone, Debug)]
pub struct GitCli {
    program: PathBuf,
}

impl GitCli {
    /// The git at `program`, used as given.
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }

    /// The first `git` on `PATH`.
    pub fn find() -> Result<Self, GitError> {
        find_program("git", &path_dirs())
            .map(Self::new)
            .ok_or_else(|| GitError::GitNotFound("no `git` on PATH".to_owned()))
    }

    pub fn program(&self) -> &Path {
        &self.program
    }

    /// Runs git with `args` in `dir` (the current directory when `None`) and
    /// returns its output whatever its exit code. Fails only when git can't be
    /// started or `cancel` fires first.
    pub fn output<I, S>(
        &self,
        dir: Option<&Path>,
        args: I,
        cancel: &CancellationToken,
    ) -> Result<GitOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.output_of(dir, &os_args(args), None, cancel)
    }

    /// `input`, when given, is written to git's stdin, which is then closed;
    /// otherwise stdin is empty.
    fn output_of(
        &self,
        dir: Option<&Path>,
        args: &[OsString],
        input: Option<Vec<u8>>,
        cancel: &CancellationToken,
    ) -> Result<GitOutput, GitError> {
        cancel.check()?;
        let mut command = Command::new(&self.program);
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        command
            .args(args)
            // Never wait for a password on a terminal nobody sees.
            .env("GIT_TERMINAL_PROMPT", "0")
            // English messages, so errors read the same everywhere.
            .env("LANGUAGE", "en")
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::configure(&mut command);

        let mut child = command.spawn().map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                GitError::GitNotFound(format!("{}: {err}", self.program.display()))
            } else {
                GitError::Backend(format!("could not run {}: {err}", self.command_line(args)))
            }
        })?;
        let stdin = feed(child.stdin.take(), input);
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());

        let mut poll = POLL_START;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(backend_error)? {
                break status;
            }
            if cancel.is_cancelled() {
                platform::kill_tree(&mut child);
                // The readers finish on their own once the pipes close.
                return Err(GitError::Cancelled);
            }
            thread::sleep(poll);
            poll = (poll * 2).min(POLL_MAX);
        };
        // shortcut: a descendant that outlives git and keeps the pipes open
        // (a daemonizing hook) blocks these joins until it exits.
        if let Some(stdin) = stdin {
            let _ = stdin.join();
        }
        Ok(GitOutput {
            exit_code: status.code(),
            stdout: join(stdout),
            stderr: join(stderr),
        })
    }

    /// Like `output`, but a non-zero exit is `GitError::CommandFailed`, which
    /// carries the exit code, stdout and stderr for the user.
    pub fn run<I, S>(
        &self,
        dir: Option<&Path>,
        args: I,
        cancel: &CancellationToken,
    ) -> Result<GitOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_of(dir, &os_args(args), None, cancel)
    }

    /// Like `run`, writing `input` to git's stdin: for data too long or too
    /// free-form for the command line, such as `--pathspec-from-file=-` or
    /// `commit --file=-`.
    pub fn run_with_input<I, S>(
        &self,
        dir: Option<&Path>,
        args: I,
        input: impl Into<Vec<u8>>,
        cancel: &CancellationToken,
    ) -> Result<GitOutput, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_of(dir, &os_args(args), Some(input.into()), cancel)
    }

    fn run_of(
        &self,
        dir: Option<&Path>,
        args: &[OsString],
        input: Option<Vec<u8>>,
        cancel: &CancellationToken,
    ) -> Result<GitOutput, GitError> {
        let output = self.output_of(dir, args, input, cancel)?;
        if output.success() {
            return Ok(output);
        }
        Err(GitError::CommandFailed {
            command: self.command_line(args),
            exit_code: output.exit_code,
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// `git <args>` for messages; the program is always shown as `git`.
    fn command_line(&self, args: &[OsString]) -> String {
        let mut line = String::from("git");
        for arg in args {
            line.push(' ');
            line.push_str(&arg.to_string_lossy());
        }
        line
    }
}

fn os_args<S: AsRef<OsStr>>(args: impl IntoIterator<Item = S>) -> Vec<OsString> {
    args.into_iter()
        .map(|arg| arg.as_ref().to_owned())
        .collect()
}

/// Writes `input` to git's stdin on its own thread, so git can fill its
/// stdout and stderr pipes meanwhile, then closes it.
fn feed(
    pipe: Option<impl Write + Send + 'static>,
    input: Option<Vec<u8>>,
) -> Option<JoinHandle<()>> {
    let (mut pipe, input) = (pipe?, input?);
    thread::Builder::new()
        .name("tafit-git-stdin".to_owned())
        .spawn(move || {
            // git may exit without reading it all; its exit code tells why.
            let _ = pipe.write_all(&input);
        })
        .ok()
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> Option<JoinHandle<Vec<u8>>> {
    let mut pipe = pipe?;
    thread::Builder::new()
        .name("tafit-git-pipe".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            // A read error leaves what was read so far; git's exit code still tells.
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
        .ok()
}

fn join(reader: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default()
}

fn backend_error(err: impl std::fmt::Display) -> GitError {
    GitError::Backend(err.to_string())
}

/// The directories on `PATH`, in order.
pub(crate) fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// The first executable called `name` in `dirs`, the way a shell would find it.
pub(crate) fn find_program(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| {
        platform::EXECUTABLE_SUFFIXES
            .iter()
            .map(|suffix| dir.join(format!("{name}{suffix}")))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(windows)]
mod platform {
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command, Stdio};

    /// Keeps a console window from flashing up for every git command.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub(super) const EXECUTABLE_SUFFIXES: &[&str] = &[".exe", ".cmd", ".bat", ".com"];

    pub(super) fn configure(command: &mut Command) {
        command.creation_flags(CREATE_NO_WINDOW);
    }

    /// Ends `child` and every process it started. The child is not reaped yet,
    /// so its PID can't have been reused.
    pub(super) fn kill_tree(child: &mut Child) {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(unix)]
mod platform {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::thread;
    use std::time::{Duration, Instant};

    use rustix::process::{Pid, Signal, kill_process_group, test_kill_process_group};

    /// How long git gets after SIGTERM to remove its lock files.
    const GRACE: Duration = Duration::from_secs(2);

    pub(super) const EXECUTABLE_SUFFIXES: &[&str] = &[""];

    /// Starts git in its own process group, so cancelling can signal
    /// everything it spawned.
    pub(super) fn configure(command: &mut Command) {
        command.process_group(0);
    }

    /// SIGTERM to git's process group, then SIGKILL to whatever is left after
    /// `GRACE`.
    pub(super) fn kill_tree(child: &mut Child) {
        let group = Pid::from_child(child);
        let _ = kill_process_group(group, Signal::TERM);
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline {
            // Until git is reaped it stays in the group as a zombie.
            let git_exited = !matches!(child.try_wait(), Ok(None));
            if git_exited && test_kill_process_group(group).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let _ = kill_process_group(group, Signal::KILL);
        let _ = child.wait();
    }
}
