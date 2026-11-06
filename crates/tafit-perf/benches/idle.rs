//! `idle_cpu/*` and `idle_rss/*`: what the store costs while nothing happens,
//! with Repositories open. Each iteration idles for a fixed time; the CPU
//! time and memory growth of the whole process are measured.

use std::thread;
use std::time::Duration;

use criterion::measurement::Measurement;
use criterion::{Criterion, SamplingMode};
use tafit_perf::harness::{self, CpuTime, Fixture, RssGrowth, Store};
use tafit_perf::{IDLE_CPU_ITERATION, IDLE_RSS_ITERATION};

fn main() {
    // The pull-request set: two benchmarks of 10 one-second iterations.
    let mut cpu = harness::criterion(CpuTime::default());
    idle(&mut cpu, "idle_cpu", &[1, 10], IDLE_CPU_ITERATION);
    if harness::nightly() {
        // Ten consecutive minutes; the warm-up minute lets startup settle.
        let mut rss = harness::criterion(RssGrowth::default());
        idle(&mut rss, "idle_rss", &[10], IDLE_RSS_ITERATION);
    }
    Criterion::default().configure_from_args().final_summary();
}

/// Benchmarks `group/<n>_repo(s)` for each count: a store with that many
/// Repositories open, idling for `iteration` at a time.
fn idle<M: Measurement>(c: &mut Criterion<M>, group: &str, counts: &[usize], iteration: Duration) {
    let mut group = c.benchmark_group(group);
    group
        .sampling_mode(SamplingMode::Flat)
        .sample_size(10)
        .warm_up_time(Duration::from_millis(1))
        // Exactly one iteration per sample: any more time and Criterion runs
        // two. It warns that the samples take slightly longer; harmless.
        .measurement_time(iteration * 10);
    for &count in counts {
        let fixtures: Vec<_> = (0..count)
            .map(|ix| Fixture::new(&format!("repo-{ix}"), 1))
            .collect();
        let store = Store::new();
        for fixture in &fixtures {
            store.open(fixture.path());
        }
        let name = if count == 1 {
            "1_repo".to_owned()
        } else {
            format!("{count}_repos")
        };
        group.bench_function(name, |b| b.iter(|| thread::sleep(iteration)));
    }
    group.finish();
}
