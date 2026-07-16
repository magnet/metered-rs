//! Compile-time proof that a `MetricTreeView<'static, C>` is `Send + Sync`
//! and can be cached in a `static`. A follow-up change caches per-type views
//! in statics, which requires every stored entry, selector, and reader
//! closure to be `Send + Sync`.

use metered_core::entry::{counter, gauge_value};
use metered_core::{Counter, MetricTreeView, MetricsView};
use std::sync::LazyLock;
use std::sync::atomic::AtomicU64;

struct Worker {
    runs: AtomicU64,
    queue: Vec<u64>,
}

impl MetricsView for Worker {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        let mut view = MetricTreeView::with_prefix("worker");
        view.register(counter("runs").select(|w: &Worker| &w.runs).help("Runs"));
        view.register(gauge_value("queue_depth").read(|w: &Worker| w.queue.len() as u64));
        view
    }
}

static WORKER_VIEW: LazyLock<MetricTreeView<'static, Worker>> = LazyLock::new(Worker::metrics_view);

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn static_view_is_send_sync_and_scrapes() {
    assert_send_sync::<MetricTreeView<'static, Worker>>();

    let worker = Worker {
        runs: AtomicU64::new(0),
        queue: vec![1, 2, 3],
    };
    Counter::incr_by(&worker.runs, 2);

    let values = WORKER_VIEW.values(&worker);
    assert!(
        values.samples().iter().any(|sample| {
            sample.name == "worker_runs_total" && sample.value.to_string() == "2"
        })
    );
    assert!(
        values.samples().iter().any(|sample| {
            sample.name == "worker_queue_depth" && sample.value.to_string() == "3"
        })
    );
}
