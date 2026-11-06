//! The budget report, fed results laid out the way Criterion writes them.

use std::path::Path;
use std::time::Duration;

use tafit_perf::{Budget, Limit, Suite, Verdict, check};
use tempfile::TempDir;

/// A Criterion output directory (`CRITERION_HOME`).
struct Results(TempDir);

impl Results {
    fn new() -> Self {
        Self(tempfile::tempdir().expect("create temp dir"))
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    /// Records `group/function` with this typical value, as Criterion does
    /// after a run: `<group>/<function>/new/{benchmark,estimates}.json`.
    fn add(&self, group: &str, function: &str, value: f64) -> &Self {
        let dir = self.path().join(group).join(function).join("new");
        std::fs::create_dir_all(&dir).expect("create result dir");
        let benchmark = format!(
            r#"{{"group_id":"{group}","function_id":"{function}","value_str":null,"throughput":null,"full_id":"{group}/{function}","directory_name":"{group}/{function}","title":"{group}/{function}"}}"#
        );
        let estimate = format!(
            r#"{{"confidence_interval":{{"confidence_level":0.95,"lower_bound":{value},"upper_bound":{value}}},"point_estimate":{value},"standard_error":0.0}}"#
        );
        let estimates = format!(
            r#"{{"mean":{estimate},"median":{estimate},"median_abs_dev":{estimate},"slope":null,"std_dev":{estimate}}}"#
        );
        std::fs::write(dir.join("benchmark.json"), benchmark).expect("write benchmark.json");
        std::fs::write(dir.join("estimates.json"), estimates).expect("write estimates.json");
        self
    }
}

const MS: f64 = 1_000_000.0;

fn open_budget() -> Budget {
    Budget {
        benchmark: "open_repository",
        limit: Limit::Time(Duration::from_millis(650)),
        suite: Suite::Pr,
    }
}

#[test]
fn a_benchmark_within_its_budget_passes() {
    let results = Results::new();
    results.add("open_repository", "fixture", 12.0 * MS);

    let report = check(results.path(), Suite::Pr, &[open_budget()]).unwrap();

    assert!(report.passed(), "{report}");
    let [row] = report.rows.as_slice() else {
        panic!("expected one row: {report}");
    };
    assert_eq!(row.benchmark, "open_repository/fixture");
    assert_eq!(row.verdict, Verdict::Within);
}

#[test]
fn a_benchmark_over_its_budget_fails_the_report() {
    let results = Results::new();
    results.add("open_repository", "fixture", 12.0 * MS).add(
        "open_repository",
        "chromium",
        900.0 * MS,
    );

    let report = check(results.path(), Suite::Pr, &[open_budget()]).unwrap();

    assert!(!report.passed(), "{report}");
    let over: Vec<_> = report
        .rows
        .iter()
        .filter(|row| row.verdict == Verdict::Over)
        .map(|row| row.benchmark.as_str())
        .collect();
    assert_eq!(over, ["open_repository/chromium"]);
    assert!(report.to_string().contains("900.0 ms"), "{report}");
}

#[test]
fn a_budget_of_the_suite_without_results_fails_the_report() {
    let results = Results::new();

    let report = check(results.path(), Suite::Pr, &[open_budget()]).unwrap();

    assert!(!report.passed(), "{report}");
    let [row] = report.rows.as_slice() else {
        panic!("expected one row: {report}");
    };
    assert_eq!(row.benchmark, "open_repository");
    assert_eq!(row.verdict, Verdict::Missing);
}

#[test]
fn a_pull_request_run_does_not_need_the_nightly_benchmarks() {
    let results = Results::new();
    results.add("open_repository", "fixture", 12.0 * MS);
    let nightly = Budget {
        benchmark: "idle_rss",
        limit: Limit::Time(Duration::from_secs(1)),
        suite: Suite::Nightly,
    };

    let pr = check(results.path(), Suite::Pr, &[open_budget(), nightly]).unwrap();
    let full = check(results.path(), Suite::Nightly, &[open_budget(), nightly]).unwrap();

    assert!(pr.passed(), "{pr}");
    assert!(
        !full.passed(),
        "the nightly run needs every benchmark: {full}"
    );
}

#[test]
fn a_run_that_wrote_no_results_reports_every_budget_missing() {
    let results = Results::new();
    let never_created = results.path().join("criterion");

    let report = check(&never_created, Suite::Pr, &[open_budget()]).unwrap();

    assert_eq!(report.rows[0].verdict, Verdict::Missing, "{report}");
}

#[test]
fn idle_cpu_is_checked_as_a_share_of_the_idle_time() {
    let results = Results::new();
    // CPU nanoseconds used per 1 s of idling: 1.5% and 2.5%.
    results
        .add("idle_cpu", "1_repo", 15.0 * MS)
        .add("idle_cpu", "10_repos", 25.0 * MS);
    let budget = Budget {
        benchmark: "idle_cpu",
        limit: Limit::CpuShare {
            max_percent: 2.0,
            iteration: Duration::from_secs(1),
        },
        suite: Suite::Pr,
    };

    let report = check(results.path(), Suite::Pr, &[budget]).unwrap();

    let verdicts: Vec<_> = report
        .rows
        .iter()
        .map(|row| (row.benchmark.as_str(), row.measured.as_str(), row.verdict))
        .collect();
    assert_eq!(
        verdicts,
        [
            ("idle_cpu/10_repos", "2.50% CPU", Verdict::Over),
            ("idle_cpu/1_repo", "1.50% CPU", Verdict::Within),
        ]
    );
}

#[test]
fn memory_growth_is_checked_over_the_budgeted_time() {
    let results = Results::new();
    // Bytes of RSS growth per 1 min of idling: 1.2 MiB and 0.8 MiB over 10 min.
    const MIB: f64 = 1024.0 * 1024.0;
    results
        .add("idle_rss_a", "10_repos", 0.12 * MIB)
        .add("idle_rss_b", "10_repos", 0.08 * MIB);
    let budget = |benchmark| Budget {
        benchmark,
        limit: Limit::MemoryGrowth {
            max_bytes: 1024 * 1024,
            over: Duration::from_secs(600),
            iteration: Duration::from_secs(60),
        },
        suite: Suite::Nightly,
    };

    let report = check(
        results.path(),
        Suite::Nightly,
        &[budget("idle_rss_a"), budget("idle_rss_b")],
    )
    .unwrap();

    let verdicts: Vec<_> = report
        .rows
        .iter()
        .map(|row| (row.measured.as_str(), row.verdict))
        .collect();
    assert_eq!(
        verdicts,
        [
            ("1.20 MiB / 10 min", Verdict::Over),
            ("0.80 MiB / 10 min", Verdict::Within),
        ]
    );
}

#[test]
fn a_budget_for_one_benchmark_leaves_its_siblings_alone() {
    let results = Results::new();
    results
        .add("idle_cpu", "1_repo", 15.0 * MS)
        .add("idle_cpu", "10_repos", 40.0 * MS);
    let share = |benchmark, max_percent| Budget {
        benchmark,
        limit: Limit::CpuShare {
            max_percent,
            iteration: Duration::from_secs(1),
        },
        suite: Suite::Pr,
    };

    let report = check(
        results.path(),
        Suite::Pr,
        &[
            share("idle_cpu/1_repo", 2.0),
            share("idle_cpu/10_repos", 5.0),
        ],
    )
    .unwrap();

    assert!(report.passed(), "{report}");
    assert_eq!(report.rows.len(), 2, "{report}");
}
