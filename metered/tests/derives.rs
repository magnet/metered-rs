//! `#[derive(LabelSet)]` (typed Family keys) and `#[derive(MetricTree)]`
//! (a struct of metrics exposed as native OpenMetrics).

use metered::{Counter, Family, Gauge, LabelSet, MetricSchema, MetricTree, MetricTreeExt};
use metered_om::{OpenMetricsEncoder, OpenMetricsExt};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};

#[derive(Clone, PartialEq, Eq, Hash, LabelSet)]
struct MethodLabels {
    method: String,
    status: u16,
}

#[derive(Default, MetricTree)]
struct AppMetrics {
    #[metric(counter)]
    requests: AtomicU64,
    #[metric]
    queue_depth: AtomicI64,
}

#[derive(Default, MetricTree)]
struct SubsystemMetrics {
    #[metric(counter)]
    jobs: AtomicU64,
}

#[derive(MetricTree)]
struct ServiceMetrics<'a> {
    #[metric]
    enabled: &'a AtomicBool,
    // Unsigned atomics must declare their exposition kind: a bare `#[metric]`
    // would leave counter-vs-gauge to the integer width.
    #[metric(gauge)]
    queue_depth: &'a AtomicUsize,
    #[metric]
    subsystem: &'a SubsystemMetrics,
}

#[test]
fn derived_label_set_keys_a_family() {
    let by_method: Family<MethodLabels, AtomicU64> = Family::default();
    by_method.with(
        &MethodLabels {
            method: "get".to_owned(),
            status: 200,
        },
        metered::Counter::incr,
    );

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        by_method.encode("requests", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(buf.contains("requests_total{method=\"get\",status=\"200\"} 1"));
}

#[test]
fn derived_encode_metric_exposes_a_struct_of_metrics() {
    let metrics = AppMetrics::default();
    metered::Counter::incr(&metrics.requests);
    metered::Gauge::set(&metrics.queue_depth, 5);

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        metrics.encode("app", &[("env", "test")], &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(buf.contains("# TYPE app_requests counter"));
    assert!(buf.contains("app_requests_total{env=\"test\"} 1"));
    assert!(buf.contains("# TYPE app_queue_depth gauge"));
    assert!(buf.contains("app_queue_depth{env=\"test\"} 5"));
}

#[derive(Default, MetricTree)]
struct PoolMetrics {
    #[metric(counter)]
    acquired: AtomicU64,
    #[metric]
    idle: AtomicI64,
}

#[derive(Default, MetricTree)]
struct RefactoredApi {
    // Renamed in code (`request_count`), but the wire name stays `requests`.
    #[metric(counter, rename = "requests")]
    request_count: AtomicU64,
    // Extracted into a sub-struct for organization, but flattened so the
    // emitted names do not gain a `pool` segment.
    #[metrics(flatten)]
    pool: PoolMetrics,
}

#[derive(Default, MetricTree)]
#[metrics(prefix = "app", label(service = "orders", region = "eu"))]
struct RootMetrics {
    #[metric(counter)]
    requests: AtomicU64,
    #[metric]
    queue_depth: AtomicI64,
}

#[test]
fn derived_metric_tree_container_attrs_apply_prefix_and_labels() {
    let metrics = RootMetrics::default();
    metered::Counter::incr(&metrics.requests);
    metered::Gauge::set(&metrics.queue_depth, 3);

    // A self-contained root renders directly, no Registry needed.
    let text = metrics.encode_to_string().unwrap();
    assert!(text.contains("# TYPE app_requests counter"));
    assert!(
        text.contains("app_requests_total{service=\"orders\",region=\"eu\"} 1"),
        "{}",
        text
    );
    assert!(text.contains("app_queue_depth{service=\"orders\",region=\"eu\"} 3"));
    assert!(text.trim_end().ends_with("# EOF"));

    // The schema carries the prefixed names and the constant labels.
    let schema = metrics.schema();
    let family = schema.family("app_requests").unwrap();
    assert!(family.labels.iter().any(|l| l == "service"));
    assert!(family.labels.iter().any(|l| l == "region"));
}

#[test]
fn derived_metric_tree_renames_and_flattens_to_keep_wire_names_stable() {
    let metrics = RefactoredApi::default();
    metrics.request_count.incr_by(3);
    metered::Counter::incr(&metrics.pool.acquired);
    metrics.pool.idle.set(7);

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        metrics.encode("api", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }

    // `rename` controls the segment.
    assert!(buf.contains("# TYPE api_requests counter"));
    assert!(buf.contains("api_requests_total 3"));
    assert!(!buf.contains("api_request_count"));

    // `flatten` drops the `pool` segment: children sit directly under `api`.
    assert!(buf.contains("api_acquired_total 1"));
    assert!(buf.contains("api_idle 7"));
    assert!(!buf.contains("api_pool_acquired"));

    // Schema and values agree on the shaped names.
    let mut schema = MetricSchema::new();
    metrics.describe("api", &[], &mut schema);
    assert!(schema.family("api_requests").is_some());
    assert!(schema.family("api_acquired").is_some());
    assert!(schema.family("api_idle").is_some());
    assert!(schema.family("api_request_count").is_none());
    assert!(schema.family("api_pool_acquired").is_none());
}

#[test]
fn derived_encode_metric_supports_borrowed_atomic_views_and_nested_trees() {
    let enabled = AtomicBool::new(true);
    let queue_depth = AtomicUsize::new(3);
    let subsystem = SubsystemMetrics::default();
    metered::Counter::incr(&subsystem.jobs);
    let metrics = ServiceMetrics {
        enabled: &enabled,
        queue_depth: &queue_depth,
        subsystem: &subsystem,
    };

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        metrics.encode("service", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }

    assert!(buf.contains("# TYPE service_enabled gauge"));
    assert!(buf.contains("service_enabled 1"));
    assert!(buf.contains("# TYPE service_queue_depth gauge"));
    assert!(buf.contains("service_queue_depth 3"));
    assert!(buf.contains("# TYPE service_subsystem_jobs counter"));
    assert!(buf.contains("service_subsystem_jobs_total 1"));

    let mut schema = MetricSchema::new();
    metrics.describe("service", &[], &mut schema);
    assert_eq!(
        schema.family("service_enabled").unwrap().metric_type,
        metered::MetricType::Gauge
    );
    assert_eq!(
        schema.family("service_queue_depth").unwrap().metric_type,
        metered::MetricType::Gauge
    );
    assert_eq!(
        schema.family("service_subsystem_jobs").unwrap().metric_type,
        metered::MetricType::Counter
    );

    enabled.store(false, Ordering::Relaxed);
    queue_depth.store(0, Ordering::Relaxed);
    let mut updated = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut updated);
        metrics.encode("service", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(updated.contains("service_enabled 0"));
    assert!(updated.contains("service_queue_depth 0"));
}
