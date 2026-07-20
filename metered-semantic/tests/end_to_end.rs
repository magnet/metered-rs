//! End-to-end: the whole model -- a `#[metered]` registry, readable primitives,
//! an adapter over existing state, a labeled `Family`, a `StateSet`, and `Info`
//! -- composed through a `Registry` into one OpenMetrics document.

use metered::{adapter, Family, InfoMetric, Registry, StateSet};
use metered_om::OpenMetricsRegistryExt;
use metered_semantic::{metered, Elapsed, HitCount};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

#[derive(Default)]
struct Worker {
    metrics: WorkerMetrics,
}

#[metered(registry = WorkerMetrics)]
impl Worker {
    #[measure([HitCount, Elapsed])]
    fn run(&self) {
        // A measurable body keeps the elapsed-sum assertion deterministic.
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn full_model_composes_into_one_openmetrics_document() {
    let worker = Worker::default();
    worker.run();
    worker.run();

    let queue_depth = AtomicI64::new(0);
    metered::Gauge::set(&queue_depth, 7);

    let enabled = AtomicBool::new(true);
    let enabled_metric = adapter::flag(|| enabled.load(Ordering::Relaxed));

    let by_route: Family<Vec<(String, String)>, AtomicU64> = Family::with_label_names(["route"]);
    by_route.with(&vec![("route".to_owned(), "/health".to_owned())], |c| {
        metered::Counter::incr(c)
    });

    let status = StateSet::new(["starting", "running", "stopped"]);
    status.set("running");

    let build = InfoMetric::new([("version", "0.10.0")]);

    let mut registry = Registry::with_prefix("worker");
    registry.label("instance", "i-1");
    registry.register(
        metered::entry::metric("ops")
            .source(&worker.metrics)
            .help("Worker operations"),
    );
    registry.register(
        metered::entry::metric("queue_depth")
            .source(&queue_depth)
            .help("Queue depth")
            .unit("items"),
    );
    registry.register(
        metered::entry::metric("enabled")
            .source(&enabled_metric)
            .help("Whether the worker is enabled"),
    );
    registry.register(
        metered::entry::metric("by_route")
            .source(&by_route)
            .help("Requests by route"),
    );
    registry.register(
        metered::entry::metric("status")
            .source(&status)
            .help("Worker status"),
    );
    registry.register(
        metered::entry::metric("build")
            .source(&build)
            .help("Build info"),
    );

    let text = registry.encode_to_string().unwrap();

    // semantic metrics via #[metered] (hierarchical names: prefix_reg_method_metric)
    assert!(text.contains("worker_ops_run_hit_count_total{instance=\"i-1\"} 2"));
    assert!(text.contains("# TYPE worker_ops_run_elapsed histogram"));
    assert!(text.contains("worker_ops_run_elapsed_count{instance=\"i-1\"} 2"));
    // The count alone survives an `observe(0.0)` regression; a positive sum
    // pins the fact that `Elapsed` measured the method's wall-clock time.
    let elapsed_sum: f64 = text
        .lines()
        .find(|line| line.starts_with("worker_ops_run_elapsed_sum{instance=\"i-1\"}"))
        .and_then(|line| line.rsplit_once(' '))
        .expect("missing worker_ops_run_elapsed_sum sample")
        .1
        .parse()
        .expect("elapsed sum parses as f64");
    assert!(elapsed_sum > 0.0);

    // readable primitive with a non-conformant unit: `items` is not an
    // `_`-separated suffix of `worker_queue_depth`, so the `# UNIT` line is
    // suppressed (it would otherwise make Prometheus reject the scrape); the
    // gauge itself still renders.
    assert!(!text.contains("# UNIT worker_queue_depth"));
    assert!(text.contains("worker_queue_depth{instance=\"i-1\"} 7"));

    // adapted existing state (a flag is a gauge 0/1)
    assert!(text.contains("# TYPE worker_enabled gauge"));
    assert!(text.contains("worker_enabled{instance=\"i-1\"} 1"));

    // dynamic labels via Family
    assert!(text.contains("worker_by_route_total{instance=\"i-1\",route=\"/health\"} 1"));

    // stateset (label key is the metric name)
    assert!(text.contains("worker_status{instance=\"i-1\",worker_status=\"running\"} 1"));
    assert!(text.contains("worker_status{instance=\"i-1\",worker_status=\"starting\"} 0"));

    // info
    assert!(text.contains("worker_build_info{instance=\"i-1\",version=\"0.10.0\"} 1"));

    // each family declared once, document terminated
    assert_eq!(
        text.matches("# TYPE worker_ops_run_hit_count counter")
            .count(),
        1
    );
    assert!(text.trim_end().ends_with("# EOF"));
}
