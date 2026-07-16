//! `#[metric(rename = "...")]` on a measured method renames its wire segment
//! while leaving the generated registry field (the Rust API) named after the
//! method -- so a method rename does not move its metrics.

use metered::{MetricSchema, MetricTree, Registry};
use metered_om::OpenMetricsRegistryExt;
use metered_semantic::{metered, HitCount};

#[derive(Default)]
struct Worker {
    metrics: WorkerMetrics,
}

#[metered(registry = WorkerMetrics)]
impl Worker {
    // Method renamed in code; the metric segment stays `run`.
    #[measure(HitCount)]
    #[metric(rename = "run")]
    fn run_iteration(&self) {}
}

#[test]
fn metered_method_rename_changes_segment_not_field() {
    let worker = Worker::default();
    worker.run_iteration();
    worker.run_iteration();

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("ops")
            .source(&worker.metrics)
            .help("Worker operations"),
    );
    let text = registry.encode_to_string().unwrap();

    // Wire segment uses the rename, not the method name.
    assert!(
        text.contains("ops_run_hit_count_total 2"),
        "expected renamed segment in:\n{}",
        text
    );
    assert!(!text.contains("run_iteration"));

    // Schema agrees on the renamed family.
    let mut schema = MetricSchema::new();
    worker.metrics.describe("ops", &[], &mut schema);
    assert!(schema.family("ops_run_hit_count").is_some());
    assert!(schema.family("ops_run_iteration_hit_count").is_none());

    // The generated registry field keeps the method name (Rust API unchanged).
    let _field = &worker.metrics.run_iteration;
}
