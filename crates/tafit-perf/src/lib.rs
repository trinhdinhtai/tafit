//! Benchmarks and the performance-budget report.
//!
//! The benchmarks in `benches/` drive the real store and backend; `check`
//! reads the results Criterion wrote and compares them with `BUDGETS`.

mod budgets;
pub mod harness;
mod report;

pub use budgets::{BUDGETS, IDLE_CPU_ITERATION, IDLE_RSS_ITERATION};
pub use report::{Budget, Limit, Report, Row, Suite, Verdict, check};
