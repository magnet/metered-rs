//! Container-level `#[metrics(crate = "...")]` path override on
//! `#[derive(MetricTree)]` and `#[derive(LabelSet)]`.
//!
//! Mirrors `metered-tracing/tests/span_labels_attrs.rs`: a local re-export
//! module stands in for a renamed facade so the derive's path substitution is
//! exercised end-to-end without a dependency cycle on the facade crate.

use metered::{Family, LabelSet, MetricTree};
use metered_om::OpenMetricsEncoder;
use std::sync::atomic::{AtomicU64, Ordering};

/// Simulated facade: generated code reaches the runtime through this
/// re-export instead of `::metered` directly.
mod facade {
    pub use metered::*;
}

#[derive(Clone, PartialEq, Eq, Hash, LabelSet)]
#[metrics(crate = "crate::facade")]
struct MethodLabels {
    method: String,
}

#[derive(Default, MetricTree)]
#[metrics(crate = "crate::facade")]
struct AppMetrics {
    #[metric(counter)]
    requests: AtomicU64,
    #[metric]
    by_method: Family<MethodLabels, AtomicU64>,
}

#[test]
fn crate_override_routes_metric_tree_and_label_set_through_the_given_path() {
    let metrics = AppMetrics::default();
    metrics.requests.fetch_add(1, Ordering::Relaxed);
    metrics.by_method.with(
        &MethodLabels {
            method: "get".to_owned(),
        },
        facade::Counter::incr,
    );

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        metrics.encode("app", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(buf.contains("app_requests_total 1"), "{buf}");
    assert!(
        buf.contains("app_by_method_total{method=\"get\"} 1"),
        "{buf}"
    );
}
