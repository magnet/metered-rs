//! The `housekeep` / `needs_housekeep` maintenance plumbing: a stub leaf metric
//! that records how often it is maintained, exercised through the trait, the
//! `#[derive(MetricTree)]` forwarding, and `Registry`'s scrape-time policy.

use metered::{
    DynamicExponentialHistogram, Metric, MetricSchema, MetricTree, MetricTreeView, MetricType,
    MetricValues, MetricsView, Registry,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A leaf metric that needs maintenance until it is housekept, counting calls.
#[derive(Default)]
struct Maintainable {
    housekeeps: AtomicUsize,
    needs: AtomicBool,
}

impl Maintainable {
    fn arm(&self) {
        self.needs.store(true, Ordering::Relaxed);
    }
    fn count(&self) -> usize {
        self.housekeeps.load(Ordering::Relaxed)
    }
}

impl Metric for Maintainable {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.gauge(name, labels, self.count() as i64);
    }

    fn needs_housekeep(&self) -> bool {
        self.needs.load(Ordering::Relaxed)
    }

    fn housekeep(&self) {
        self.housekeeps.fetch_add(1, Ordering::Relaxed);
        self.needs.store(false, Ordering::Relaxed);
    }
}

#[test]
fn registry_housekeeps_on_scrape_by_default() {
    let metric = Maintainable::default();
    metric.arm();

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("widget")
            .source(&metric)
            .help("A maintainable widget"),
    );

    assert_eq!(metric.count(), 0);
    let _ = registry.values(); // scrape runs maintenance by default
    assert_eq!(metric.count(), 1);
    assert!(
        !Metric::needs_housekeep(&metric),
        "housekeep cleared the flag"
    );

    // Nothing due now -> the sweep skips it (needs_housekeep gates the call).
    let _ = registry.values();
    assert_eq!(metric.count(), 1);
}

#[test]
fn housekeep_on_scrape_can_be_disabled_and_driven_manually() {
    let metric = Maintainable::default();
    metric.arm();

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("widget")
            .source(&metric)
            .help("A maintainable widget"),
    );
    registry.housekeep_on_scrape(false);

    let _ = registry.values();
    assert_eq!(metric.count(), 0, "scrape must not maintain when disabled");

    registry.housekeep(); // driven by an external maintenance task
    assert_eq!(metric.count(), 1);
}

#[test]
fn needs_housekeep_propagates_through_derive_and_forwards_housekeep() {
    #[derive(Default, MetricTree)]
    #[metrics(prefix = "app")]
    struct AppMetrics {
        #[metric]
        widget: Maintainable,
        // A plain field with no upkeep must not flip the tree's flag.
        #[allow(dead_code)]
        counter: std::sync::atomic::AtomicU64,
    }

    let metrics = AppMetrics::default();
    assert!(
        !metrics.needs_housekeep(),
        "nothing armed -> tree reports no upkeep due"
    );

    metrics.widget.arm();
    assert!(
        metrics.needs_housekeep(),
        "an armed child propagates needs_housekeep up the derived tree"
    );

    // Forwarding reaches the nested leaf.
    metrics.housekeep();
    assert_eq!(metrics.widget.count(), 1);
    assert!(!metrics.needs_housekeep());

    // Sanity: the derived tree still describes/collects normally.
    let mut schema = MetricSchema::new();
    metrics.describe("", &[], &mut schema);
    assert!(schema.family("app_widget").is_some());
}

#[test]
fn needs_housekeep_reaches_a_metric_behind_a_view_field() {
    // A component that exposes its metrics through the `MetricsView` seam, with a
    // dynamic exponential histogram that needs rescaling once it saturates.
    struct Rpc {
        latency: DynamicExponentialHistogram,
    }

    impl MetricsView for Rpc {
        fn metrics_view() -> MetricTreeView<'static, Self> {
            let mut view = MetricTreeView::new();
            view.register(metered::entry::metric("latency").select(|rpc: &Rpc| &rpc.latency));
            view
        }
    }

    // The histogram is reachable ONLY through a `#[metrics]` (view) field. Before
    // the fix, a view field contributed nothing to the tree's `needs_housekeep`,
    // so the gate skipped housekeep and the histogram never rescaled once
    // saturated.
    #[derive(MetricTree)]
    #[metrics(prefix = "app")]
    struct AppMetrics {
        #[metrics(view)]
        rpc: Rpc,
    }

    let metrics = AppMetrics {
        rpc: Rpc {
            latency: DynamicExponentialHistogram::with_params(6, 16),
        },
    };
    let start_schema = metrics.rpc.latency.schema();

    // Nothing due yet -> the tree reports no upkeep.
    assert!(
        !metrics.needs_housekeep(),
        "a clean view subtree reports no upkeep"
    );

    // Saturate the histogram so a downscale is pending.
    for k in 0..4000 {
        metrics.rpc.latency.observe(1e-5 * 1.03f64.powi(k % 400));
        if metrics.rpc.latency.needs_rescale() {
            break;
        }
    }
    assert!(
        metrics.rpc.latency.needs_rescale(),
        "precondition: the histogram needs a rescale"
    );

    // The regression: a maintainable metric behind a view field must flip the
    // derived tree's flag, or every housekeep gate skips it.
    assert!(
        metrics.needs_housekeep(),
        "needs_housekeep must propagate through a #[metrics] view field"
    );

    // And housekeep reaches through the view field to actually rescale it.
    metrics.housekeep();
    assert!(
        metrics.rpc.latency.schema() < start_schema,
        "housekeep must rescale the histogram behind the view field"
    );
}
