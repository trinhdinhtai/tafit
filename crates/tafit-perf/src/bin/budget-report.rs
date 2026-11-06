//! `budget-report [--nightly] [<criterion-dir>]`: checks the latest Criterion
//! results against tafit's budgets and exits non-zero when one is exceeded or
//! a benchmark of the run is missing.
//!
//! The directory defaults to `$CRITERION_HOME`, else `target/criterion`. The
//! report is a Markdown table; in GitHub Actions it is also added to the job
//! summary.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use tafit_perf::{BUDGETS, Suite, check};

fn main() -> ExitCode {
    let mut suite = Suite::Pr;
    let mut dir = None;
    for arg in std::env::args_os().skip(1) {
        if arg == "--nightly" {
            suite = Suite::Nightly;
        } else if dir.is_none() && !arg.to_string_lossy().starts_with('-') {
            dir = Some(PathBuf::from(arg));
        } else {
            eprintln!("usage: budget-report [--nightly] [<criterion-dir>]");
            return ExitCode::FAILURE;
        }
    }
    let dir = dir
        .or_else(|| std::env::var_os("CRITERION_HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("target/criterion"));

    let report = match check(&dir, suite, BUDGETS) {
        Ok(report) => report,
        Err(err) => {
            eprintln!("cannot read Criterion results in {}: {err}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    let heading = if report.passed() {
        "Performance budgets: all within budget"
    } else {
        "Performance budgets: FAILED"
    };
    let text = format!("## {heading}\n\n{report}");
    print!("{text}");
    if let Some(summary) = std::env::var_os("GITHUB_STEP_SUMMARY") {
        let appended = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(summary)
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(err) = appended {
            eprintln!("cannot write the job summary: {err}");
        }
    }
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
