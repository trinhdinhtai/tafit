//! tafit's performance budgets. A ticket that adds a benchmark adds its budget
//! here.

use std::time::Duration;

use crate::{Budget, Limit, Suite};

/// How long one iteration of the idle CPU benchmarks idles.
pub const IDLE_CPU_ITERATION: Duration = Duration::from_secs(1);

/// How long one iteration of the idle memory benchmark idles. Ten iterations
/// make the ten minutes the budget is stated over.
pub const IDLE_RSS_ITERATION: Duration = Duration::from_secs(60);

pub const BUDGETS: &[Budget] = &[
    // Pull requests open a generated fixture; the nightly run adds real
    // Repository snapshots to the same group.
    Budget {
        benchmark: "open_repository",
        limit: Limit::Time(Duration::from_millis(650)),
        suite: Suite::Pr,
    },
    // One refresh of an 80,000-file Repository with changes. About 270 ms on
    // a Windows dev machine; tighten once CI's numbers settle. The nightly
    // snapshots run unbudgeted until they have a few nights of results.
    Budget {
        benchmark: "status/80k_files",
        limit: Limit::Time(Duration::from_millis(400)),
        suite: Suite::Pr,
    },
    Budget {
        benchmark: "idle_cpu/1_repo",
        limit: Limit::CpuShare {
            max_percent: 2.0,
            iteration: IDLE_CPU_ITERATION,
        },
        suite: Suite::Pr,
    },
    Budget {
        benchmark: "idle_cpu/10_repos",
        limit: Limit::CpuShare {
            max_percent: 5.0,
            iteration: IDLE_CPU_ITERATION,
        },
        suite: Suite::Pr,
    },
    Budget {
        benchmark: "idle_rss",
        limit: Limit::MemoryGrowth {
            max_bytes: 1024 * 1024,
            over: Duration::from_secs(10 * 60),
            iteration: IDLE_RSS_ITERATION,
        },
        suite: Suite::Nightly,
    },
];
