//! Reads Criterion's results and checks them against performance budgets.

use std::fmt;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

/// Which run a budget belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Suite {
    /// The small set every pull request runs.
    Pr,
    /// The full set run every night on real Repository snapshots.
    Nightly,
}

/// The most a benchmark may take, and how to read its results.
#[derive(Clone, Copy, Debug)]
pub enum Limit {
    /// Wall time per iteration.
    Time(Duration),
    /// CPU time per iteration, as a share of the iteration's wall time. The
    /// benchmark measures CPU nanoseconds while idling for `iteration`.
    CpuShare {
        max_percent: f64,
        iteration: Duration,
    },
    /// Resident memory growth over `over`. The benchmark measures the bytes
    /// gained while idling for `iteration`; consecutive iterations add up.
    MemoryGrowth {
        max_bytes: u64,
        over: Duration,
        iteration: Duration,
    },
}

impl Limit {
    /// One iteration's `value` (in the benchmark's unit) and the limit, both
    /// in the unit the limit is stated in.
    fn measure(&self, value: f64) -> (f64, f64) {
        match *self {
            Limit::Time(max) => (value, max.as_nanos() as f64),
            Limit::CpuShare {
                max_percent,
                iteration,
            } => (value / iteration.as_nanos() as f64 * 100.0, max_percent),
            Limit::MemoryGrowth {
                max_bytes,
                over,
                iteration,
            } => (
                value * (over.as_secs_f64() / iteration.as_secs_f64()),
                max_bytes as f64,
            ),
        }
    }

    /// An amount in the limit's unit, as the report shows it.
    fn format(&self, amount: f64) -> String {
        match *self {
            Limit::Time(_) => format_time(amount),
            Limit::CpuShare { .. } => format!("{amount:.2}% CPU"),
            Limit::MemoryGrowth { over, .. } => format!(
                "{:.2} MiB / {} min",
                amount / (1024.0 * 1024.0),
                over.as_secs() / 60
            ),
        }
    }
}

/// A performance budget.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// A Criterion benchmark id (`idle_cpu/1_repo`), or a group
    /// (`open_repository`) to cover every benchmark in it.
    pub benchmark: &'static str,
    pub limit: Limit,
    pub suite: Suite,
}

impl Budget {
    /// Whether a run of `suite` must produce this budget's benchmarks. The
    /// nightly run includes the pull-request set.
    fn runs_in(&self, suite: Suite) -> bool {
        self.suite == Suite::Pr || suite == Suite::Nightly
    }

    fn covers(&self, id: &str) -> bool {
        id.strip_prefix(self.benchmark)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }
}

/// How a benchmark did against its budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Within,
    Over,
    /// The budget belongs to this run, but none of its benchmarks ran.
    Missing,
}

/// One benchmark checked against its budget.
#[derive(Clone, Debug)]
pub struct Row {
    /// Criterion's full id, e.g. `open_repository/fixture`.
    pub benchmark: String,
    pub measured: String,
    pub limit: String,
    pub verdict: Verdict,
}

/// The run checked against its budgets: missing benchmarks first, then
/// every result a budget covers, by id.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub rows: Vec<Row>,
}

impl Report {
    /// Whether every benchmark stayed within its budget.
    pub fn passed(&self) -> bool {
        self.rows.iter().all(|row| row.verdict == Verdict::Within)
    }
}

/// Reads the latest results under `criterion_dir` (Criterion's
/// `CRITERION_HOME`) and checks each one against the first budget that covers
/// it. Results no budget covers are left out.
pub fn check(criterion_dir: &Path, suite: Suite, budgets: &[Budget]) -> io::Result<Report> {
    let results = read_results(criterion_dir)?;
    let mut rows = Vec::new();
    for budget in budgets.iter().filter(|b| b.runs_in(suite)) {
        if !results.iter().any(|r| budget.covers(&r.id)) {
            rows.push(Row {
                benchmark: budget.benchmark.to_owned(),
                measured: "no result".to_owned(),
                limit: budget.limit.to_string(),
                verdict: Verdict::Missing,
            });
        }
    }
    for result in results {
        let Some(budget) = budgets.iter().find(|b| b.covers(&result.id)) else {
            continue;
        };
        let (measured, max) = budget.limit.measure(result.value);
        let verdict = if measured <= max {
            Verdict::Within
        } else {
            Verdict::Over
        };
        rows.push(Row {
            benchmark: result.id,
            measured: budget.limit.format(measured),
            limit: budget.limit.to_string(),
            verdict,
        });
    }
    Ok(Report { rows })
}

/// One benchmark's latest result.
struct BenchResult {
    id: String,
    /// Criterion's typical estimate, in the measurement's unit.
    value: f64,
}

#[derive(Deserialize)]
struct BenchmarkFile {
    full_id: String,
}

#[derive(Deserialize)]
struct EstimatesFile {
    mean: Estimate,
    slope: Option<Estimate>,
}

#[derive(Deserialize)]
struct Estimate {
    point_estimate: f64,
}

/// Every `new/` result below `dir`, sorted by id.
fn read_results(dir: &Path) -> io::Result<Vec<BenchResult>> {
    let mut results = Vec::new();
    match collect_results(dir, &mut results) {
        // Nothing ran yet: every budget of the run is missing.
        Err(err) if err.kind() == io::ErrorKind::NotFound && !dir.exists() => {}
        other => other?,
    }
    results.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(results)
}

fn collect_results(dir: &Path, results: &mut Vec<BenchResult>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|name| name == "new") {
            if let Some(result) = read_result(&path)? {
                results.push(result);
            }
        } else {
            collect_results(&path, results)?;
        }
    }
    Ok(())
}

/// The result in a `new/` directory; `None` when it isn't a benchmark's.
fn read_result(dir: &Path) -> io::Result<Option<BenchResult>> {
    let benchmark = dir.join("benchmark.json");
    let estimates = dir.join("estimates.json");
    if !benchmark.is_file() || !estimates.is_file() {
        return Ok(None);
    }
    let benchmark: BenchmarkFile = read_json(&benchmark)?;
    let estimates: EstimatesFile = read_json(&estimates)?;
    // Linear sampling gives a slope; flat sampling only a mean.
    let typical = estimates.slope.unwrap_or(estimates.mean);
    Ok(Some(BenchResult {
        id: benchmark.full_id,
        value: typical.point_estimate,
    }))
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {err}", path.display()),
        )
    })
}

fn format_time(nanos: f64) -> String {
    if nanos < 1_000_000.0 {
        format!("{:.1} µs", nanos / 1_000.0)
    } else {
        format!("{:.1} ms", nanos / 1_000_000.0)
    }
}

impl fmt::Display for Report {
    /// A Markdown table, for the terminal and the CI job summary.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "| Benchmark | Measured | Budget | |")?;
        writeln!(f, "|---|---:|---:|---|")?;
        for row in &self.rows {
            let verdict = match row.verdict {
                Verdict::Within => "ok",
                Verdict::Over => "OVER BUDGET",
                Verdict::Missing => "MISSING",
            };
            writeln!(
                f,
                "| {} | {} | {} | {verdict} |",
                row.benchmark, row.measured, row.limit
            )?;
        }
        Ok(())
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Limit::Time(max) => f.write_str(&self.format(max.as_nanos() as f64)),
            Limit::CpuShare { max_percent, .. } => f.write_str(&self.format(*max_percent)),
            Limit::MemoryGrowth { max_bytes, .. } => f.write_str(&self.format(*max_bytes as f64)),
        }
    }
}
