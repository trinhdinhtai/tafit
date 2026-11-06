//! `open_repository/*`: from `Msg::OpenRepo` until the Repository Tab has
//! loaded, through the store and the gix backend.

use std::path::Path;

use criterion::{BatchSize, Criterion};
use tafit_perf::harness::{self, Fixture, Store};

fn main() {
    let mut c = harness::criterion(criterion::measurement::WallTime);
    let mut group = c.benchmark_group("open_repository");
    // Opening a large Repository takes a good part of a second.
    group.sample_size(20);

    let fixture = Fixture::new("fixture", 10);
    bench_open(&mut group, "fixture", fixture.path());
    for (name, path) in harness::snapshots() {
        bench_open(&mut group, name, &path);
    }
    group.finish();
    Criterion::default().configure_from_args().final_summary();
}

fn bench_open(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &str,
    path: &Path,
) {
    group.bench_function(name, |b| {
        // A fresh store each time, as at startup; building it isn't timed.
        b.iter_batched(
            Store::new,
            |store| {
                store.open(path);
                store
            },
            BatchSize::PerIteration,
        );
    });
}
