//! `history/*`: building the history index of a whole Repository, from
//! `Msg::RefreshHistory` until the new index is published; and reading one
//! window of commit metadata in the middle of it, from `Msg::ShowHistoryRows`
//! until the rows are published. Pull requests read a generated
//! 100,000-commit history; the nightly run adds the real Repository
//! snapshots.

use std::path::Path;

use criterion::Criterion;
use criterion::measurement::WallTime;
use tafit_perf::harness::{self, Fixture, Store};
use tafit_state::{HISTORY_WINDOW, history_window, history_window_rows};

fn main() {
    let mut c = harness::criterion(WallTime);
    let mut group = c.benchmark_group("history");
    group.sample_size(10);

    let fixture = Fixture::history("fixture", 100_000);
    bench_history(&mut group, "100k_commits", fixture.path());
    for (name, path) in harness::snapshots() {
        bench_history(&mut group, name, &path);
    }
    group.finish();
    Criterion::default().configure_from_args().final_summary();
}

fn bench_history(group: &mut criterion::BenchmarkGroup<'_, WallTime>, name: &str, path: &Path) {
    let store = Store::new();
    let (repo_id, len) = store.open_with_history(path);
    group.bench_function(format!("index/{name}"), |b| {
        b.iter(|| store.refresh_history(repo_id));
    });
    // Two windows far apart, a quarter and three quarters down: showing one
    // drops the other, so every iteration reads a window. A history too
    // short for that has no window worth measuring.
    let (near, far) = (
        history_window_rows(history_window(len / 4)),
        history_window_rows(history_window(len * 3 / 4)),
    );
    let far = far.start..far.end.min(len);
    if far.start <= near.end + 8 * HISTORY_WINDOW {
        return;
    }
    let mut flip = false;
    group.bench_function(format!("window/{name}"), |b| {
        b.iter(|| {
            flip = !flip;
            store.show_history_rows(repo_id, if flip { far.clone() } else { near.clone() });
        });
    });
}
