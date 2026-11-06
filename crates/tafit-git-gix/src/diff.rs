//! Line Diffs from `git diff`, which gix can't produce as patches yet. git is
//! always called with a fixed config, so the user's can't change the output
//! (ADR-0002).

use std::path::Path;

use tafit_core::{CancellationToken, DiffSource, FileDiff, GitError};

use crate::cli::GitCli;

/// Overrides every setting that would change what is parsed: an external
/// diff, color, quoted names, other prefixes, a textconv filter, and a blank
/// context line printed without its space.
const DIFF_ARGS: &[&str] = &[
    "diff",
    "--no-ext-diff",
    "--no-color",
    "--no-textconv",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

pub(crate) fn diff(
    git: &GitCli,
    workdir: &Path,
    source: DiffSource,
    paths: &[String],
    cancel: &CancellationToken,
) -> Result<FileDiff, GitError> {
    let mut args: Vec<&str> = vec![
        "-c",
        "core.quotepath=false",
        "-c",
        "diff.suppressBlankEmpty=false",
    ];
    if source != DiffSource::Staged {
        // Reading the work tree must not write the index behind the user's
        // own git (a refreshed index is an optional lock).
        args.extend(["--no-optional-locks", "-c", "diff.autoRefreshIndex=false"]);
    }
    let output = match source {
        DiffSource::Staged | DiffSource::Unstaged => {
            // A name like `*.txt` is a file, not a glob.
            args.insert(0, "--literal-pathspecs");
            args.extend(DIFF_ARGS);
            if source == DiffSource::Staged {
                args.push("--cached");
            }
            // Two paths are a rename's sides; show it as one.
            args.extend(["--find-renames", "--"]);
            args.extend(paths.iter().map(String::as_str));
            git.run(Some(workdir), &args, cancel)?
        }
        DiffSource::Untracked => {
            args.extend(DIFF_ARGS);
            args.extend(["--no-index", "--", "/dev/null"]);
            args.extend(paths.iter().map(String::as_str));
            let output = git.output(Some(workdir), &args, cancel)?;
            // `--no-index` exits with 1 when the files differ, as they do.
            if !matches!(output.exit_code, Some(0 | 1)) {
                return Err(GitError::CommandFailed {
                    command: format!("git {}", args.join(" ")),
                    exit_code: output.exit_code,
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
            output
        }
    };
    Ok(FileDiff::parse(&output.stdout))
}
