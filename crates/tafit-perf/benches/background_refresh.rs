//! `background_refresh/10k_files`: from a file written outside the app
//! until the watch has the new change list published, through the file
//! watcher, the store and the gix backend. The watch's quiet spell (250 ms
//! in the app) is a wait, not work, so it is cut to a few milliseconds here.

use std::time::Duration;

use criterion::Criterion;
use tafit_perf::harness::{self, Fixture, Store};
use tafit_state::WatchOptions;

fn main() {
    let mut c = harness::criterion(criterion::measurement::WallTime);
    let mut group = c.benchmark_group("background_refresh");
    group.sample_size(20);

    let fixture = Fixture::large("fixture", 10_000);
    let store = Store::with_watch_options(WatchOptions {
        debounce: Duration::from_millis(5),
        ..WatchOptions::default()
    });
    let repo_id = store.open_with_status(fixture.path());
    store.wait_until_watched(repo_id);

    // A new file each time, removing the one before, so each wait is for
    // this write's re-read and not a late one of the last.
    let mut next: usize = 0;
    group.bench_function("10k_files", |b| {
        b.iter(|| {
            let name = format!("outside-{next}.txt");
            let previous = next.checked_sub(1).map(|ix| format!("outside-{ix}.txt"));
            store.write_outside(repo_id, fixture.path(), &name, previous.as_deref());
            next += 1;
        });
    });
    group.finish();
    Criterion::default().configure_from_args().final_summary();
}
