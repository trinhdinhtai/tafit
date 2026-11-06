//! `status/*`: from `Msg::RefreshStatus` until the new change list is
//! published, through the store and the gix backend. Pull requests read a
//! generated 80,000-file Repository with changes; the nightly run adds the
//! real Repository snapshots.

use std::path::Path;

use criterion::Criterion;
use criterion::measurement::WallTime;
use tafit_perf::harness::{self, Fixture, Store};

fn main() {
    let mut c = harness::criterion(WallTime);
    let mut group = c.benchmark_group("status");
    group.sample_size(20);

    let fixture = Fixture::large("fixture", 80_000);
    bench_status(&mut group, "80k_files", fixture.path());
    for (name, path) in harness::snapshots() {
        bench_status(&mut group, name, &path);
    }
    group.finish();
    Criterion::default().configure_from_args().final_summary();
}

fn bench_status(group: &mut criterion::BenchmarkGroup<'_, WallTime>, name: &str, path: &Path) {
    // One open Repository, refreshed over and over, as with the Refresh button.
    let store = Store::new();
    let repo_id = store.open_with_status(path);
    group.bench_function(name, |b| b.iter(|| store.refresh_status(repo_id)));
}
