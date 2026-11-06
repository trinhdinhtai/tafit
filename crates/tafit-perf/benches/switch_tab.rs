//! `switch_tab/20_tabs`: from `Msg::ActivateRepo` until the selected
//! Repository Tab is published as the Active Repository, with 20 Repository
//! Tabs open and loaded, through the store and the gix backend.

use criterion::Criterion;
use tafit_perf::harness::{self, Fixture, Store};

const TABS: usize = 20;

fn main() {
    let mut c = harness::criterion(criterion::measurement::WallTime);
    let mut group = c.benchmark_group("switch_tab");

    let fixtures: Vec<_> = (0..TABS)
        .map(|ix| Fixture::new(&format!("repo-{ix}"), 1))
        .collect();
    let store = Store::new();
    // Each with its change list, so no read lands while switching.
    let tabs: Vec<_> = fixtures
        .iter()
        .map(|fixture| store.open_with_status(fixture.path()))
        .collect();

    // Round the tabs, so each switch selects a tab that isn't active.
    let mut next = 0;
    group.bench_function(format!("{TABS}_tabs"), |b| {
        b.iter(|| {
            store.activate(tabs[next]);
            next = (next + 1) % TABS;
        });
    });
    group.finish();
    Criterion::default().configure_from_args().final_summary();
}
