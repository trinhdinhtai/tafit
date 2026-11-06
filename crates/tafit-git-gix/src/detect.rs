//! Finds the system git, its version, and the signing tools it calls.

use std::path::{Path, PathBuf};

use tafit_core::{CancellationToken, GitEnvironment, GitError, GitVersion};

use crate::cli::{GitCli, find_program, path_dirs};

pub(crate) fn detect_git(
    cli: &GitCli,
    cancel: &CancellationToken,
) -> Result<GitEnvironment, GitError> {
    let output = cli.run(None, ["--version"], cancel)?;
    let printed = if output.stdout.trim_ascii().is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    let printed = String::from_utf8_lossy(printed);
    let version = GitVersion::parse(&printed).ok_or_else(|| {
        GitError::Backend(format!(
            "could not read the version from `git --version`: {}",
            printed.trim()
        ))
    })?;

    let programs = signing_programs(cli, cancel)?;
    let search = tool_search_dirs(cli.program());
    Ok(GitEnvironment {
        git_path: cli.program().to_owned(),
        version,
        gpg_path: resolve(&programs.gpg, &search),
        ssh_keygen_path: resolve(&programs.ssh_keygen, &search),
    })
}

struct SigningPrograms {
    gpg: String,
    ssh_keygen: String,
}

/// `gpg.program` and `gpg.ssh.program` from the user's and system's config, or
/// git's defaults. A Repository's own config is ignored: this describes the
/// machine, not one Repository.
fn signing_programs(cli: &GitCli, cancel: &CancellationToken) -> Result<SigningPrograms, GitError> {
    let mut programs = SigningPrograms {
        gpg: "gpg".to_owned(),
        ssh_keygen: "ssh-keygen".to_owned(),
    };
    let args = [
        "config",
        "--show-scope",
        "--get-regexp",
        r"^gpg\.(program|ssh\.program)$",
    ];
    let output = match cli.output(None, args, cancel) {
        Ok(output) => output,
        Err(GitError::Cancelled) => return Err(GitError::Cancelled),
        // Unreadable config: git would fall back to the defaults too.
        Err(_) => return Ok(programs),
    };
    // Lines look like `global\tgpg.program /usr/bin/gpg`, system scope first,
    // so a later line overrides an earlier one, as in git.
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((scope, entry)) = line.split_once('\t') else {
            continue;
        };
        if !matches!(scope, "global" | "system") {
            continue;
        }
        match entry.split_once(' ') {
            Some(("gpg.program", value)) => programs.gpg = value.to_owned(),
            Some(("gpg.ssh.program", value)) => programs.ssh_keygen = value.to_owned(),
            _ => {}
        }
    }
    Ok(programs)
}

/// Where git looks for the programs it runs: `PATH`, after the tools that Git
/// for Windows bundles next to itself (it puts those first on `PATH`).
fn tool_search_dirs(git: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(windows) {
        // git.exe sits in `<root>\cmd`, `<root>\bin` or `<root>\mingw64\bin`.
        if let Some(root) = git
            .ancestors()
            .skip(1)
            .take(3)
            .find(|dir| dir.join("usr").join("bin").is_dir())
        {
            dirs.push(root.join("mingw64").join("bin"));
            dirs.push(root.join("usr").join("bin"));
        }
    }
    dirs.extend(path_dirs());
    dirs
}

/// A configured program: a path is taken as is, a bare name is searched for.
fn resolve(program: &str, search: &[PathBuf]) -> Option<PathBuf> {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file().then(|| path.to_owned());
    }
    find_program(program, search)
}
