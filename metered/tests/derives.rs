//! `#[derive(LabelSet)]` (typed Family keys) and `#[derive(MetricTree)]`
//! (a struct of metrics exposed as native OpenMetrics).

use metered::{
    AsInfo, Counter, Family, Gauge, InfoMetric, LabelSet, MetricSchema, MetricTree, MetricTreeExt,
    MetricTreeView, MetricValues, MetricsView, Renamed,
};
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
fn derived_label_set_family_scrape_output_is_stable() {
    let by_method: Family<MethodLabels, AtomicU64> = Family::default();
    for (method, status, hits) in [("get", 200u16, 2u64), ("get", 500, 1), ("post", 200, 3)] {
        let key = MethodLabels {
            method: method.to_owned(),
            status,
        };
        by_method.with(&key, |c| c.incr_by(hits));
    }

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        by_method.encode("requests", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }

    // Byte-exact: label names/values render per the derive's `Display`
    // semantics and series sort by their label pairs.
    assert_eq!(
        buf,
        "# TYPE requests counter\n\
         requests_total{method=\"get\",status=\"200\"} 2\n\
         requests_total{method=\"get\",status=\"500\"} 1\n\
         requests_total{method=\"post\",status=\"200\"} 3\n\
         # EOF\n"
    );
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

#[derive(Default, MetricTree)]
#[metrics(prefix = "app", label(service = "workers"))]
struct ShadowingRoot {
    #[metric(counter)]
    requests: AtomicU64,
}

/// A container `#[metrics(label(...))]` pair colliding with an enclosing
/// (registry) constant label must resolve inner-wins -- exactly one pair per
/// label name on the wire -- not append a duplicate name.
#[test]
fn derived_container_label_shadows_enclosing_label_inner_wins() {
    let metrics = ShadowingRoot::default();
    metered::Counter::incr(&metrics.requests);
    let outer = [("service", "registry"), ("region", "eu")];

    // Values/wire: one `service` pair, the container's value, in the outer
    // pair's position (deterministic label order).
    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        metrics.encode("", &outer, &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(
        buf.contains("app_requests_total{service=\"workers\",region=\"eu\"} 1"),
        "{buf}"
    );
    assert_eq!(
        buf.matches("service=").count(),
        1,
        "the shadowed outer pair must drop out, not duplicate: {buf}"
    );

    // Schema: the family declares `service` once.
    let mut schema = MetricSchema::new();
    metrics.describe("", &outer, &mut schema);
    let family = schema.family("app_requests").unwrap();
    assert_eq!(
        family.labels.iter().filter(|l| *l == "service").count(),
        1,
        "schema labels: {:?}",
        family.labels
    );
}

#[derive(MetricTree)]
struct BuildTree {
    #[metrics(info)]
    build: InfoMetric,
}

/// An `Info` field's intrinsic label colliding with an enclosing label must
/// likewise resolve inner-wins in both schema and values.
#[test]
fn derived_info_intrinsic_label_shadows_enclosing_label_inner_wins() {
    let metrics = BuildTree {
        build: InfoMetric::new([("service", "workers"), ("version", "1.2.3")]),
    };
    let outer = [("service", "registry")];

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        metrics.encode("app", &outer, &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(
        buf.contains("app_build_info{service=\"workers\",version=\"1.2.3\"} 1"),
        "{buf}"
    );
    assert_eq!(
        buf.matches("service=").count(),
        1,
        "the shadowed outer pair must drop out, not duplicate: {buf}"
    );

    let mut schema = MetricSchema::new();
    metrics.describe("app", &outer, &mut schema);
    let family = schema.family("app_build").unwrap();
    assert_eq!(family.metric_type, metered::MetricType::Info);
    assert_eq!(
        family.labels.iter().filter(|l| *l == "service").count(),
        1,
        "schema labels: {:?}",
        family.labels
    );
}

/// Hand-written trees can force the same `info` leaf path the derive emits
/// by wrapping an `Info` value in the public `AsInfo` adapter.
#[test]
fn hand_written_tree_uses_as_info_directly() {
    struct Build {
        build: InfoMetric,
    }

    impl MetricTree for Build {
        fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
            let wrapped = AsInfo::from(&self.build);
            let node = Renamed::new("build", &wrapped);
            MetricTree::describe(&node, name, labels, schema);
        }

        fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
            let wrapped = AsInfo::from(&self.build);
            let node = Renamed::new("build", &wrapped);
            MetricTree::collect(&node, name, labels, values);
        }
    }

    let metrics = Build {
        build: InfoMetric::new([("service", "workers"), ("version", "1.2.3")]),
    };
    let outer = [("service", "registry")];

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        metrics.encode("app", &outer, &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(
        buf.contains("app_build_info{service=\"workers\",version=\"1.2.3\"} 1"),
        "{buf}"
    );
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

/// Counts how many times its `metrics_view()` layout is constructed, so the
/// derive's per-process view cache is observable.
struct CountedComponent {
    hits: AtomicU64,
}

static COUNTED_VIEW_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

impl MetricsView for CountedComponent {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        COUNTED_VIEW_CONSTRUCTIONS.fetch_add(1, Ordering::Relaxed);
        let mut view = MetricTreeView::new();
        view.register(metered::entry::counter("hits").select(|c: &CountedComponent| &c.hits));
        view
    }
}

#[derive(MetricTree)]
struct CountedTree {
    #[metrics]
    component: CountedComponent,
}

#[test]
fn derived_view_field_builds_its_layout_at_most_once_per_process() {
    let tree = CountedTree {
        component: CountedComponent {
            hits: AtomicU64::new(4),
        },
    };

    // Several full scrape cycles across all four traversals.
    for _ in 0..3 {
        let mut schema = MetricSchema::new();
        tree.describe("app", &[], &mut schema);
        let mut values = MetricValues::new();
        tree.collect("app", &[], &mut values);
        tree.housekeep();
        let _ = tree.needs_housekeep();
    }

    assert_eq!(
        COUNTED_VIEW_CONSTRUCTIONS.load(Ordering::Relaxed),
        1,
        "the view layout must be built exactly once and cached across \
         describe/collect/housekeep/needs_housekeep"
    );

    // The cached layout still scrapes correctly.
    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        tree.encode("app", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(buf.contains("app_component_hits_total 4"), "{buf}");
}

/// A view field whose type is the struct's own generic parameter: a `static`
/// cannot mention `T`, so the derive falls back to per-call view construction
/// for this field.
#[derive(MetricTree)]
struct GenericTree<T: MetricsView + 'static> {
    #[metrics]
    inner: T,
}

#[test]
fn derived_view_field_on_a_generic_struct_compiles_via_the_fallback() {
    struct Job {
        done: AtomicU64,
    }

    impl MetricsView for Job {
        fn metrics_view() -> MetricTreeView<'static, Self> {
            let mut view = MetricTreeView::new();
            view.register(metered::entry::counter("done").select(|j: &Job| &j.done));
            view
        }
    }

    let tree = GenericTree {
        inner: Job {
            done: AtomicU64::new(2),
        },
    };

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        tree.encode("jobs", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(buf.contains("jobs_inner_done_total 2"), "{buf}");
}
