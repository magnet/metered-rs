//! Rendering of `metered`'s core trees and adapters through the OpenMetrics
//! sink. These live here rather than in `metered`'s own unit tests because they
//! exercise the `metered-om` encoder (which depends on `metered`); a
//! crate's unit tests cannot pull a dependency that depends back on it.

use metered::adapter::{flag, CounterFn};
use metered::entry::{metric, tree};
use metered::shape::{Flatten, Renamed};
use metered::{
    join_name, BucketHistogram, Counter, DynamicExponentialHistogram, Family,
    FixedExponentialHistogram, MetricSchema, MetricTree, MetricType, MetricValues, Registry,
};
use metered_om::{HistogramProfile, OpenMetricsEncoder, OpenMetricsRegistryExt};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

fn encode(registry: &Registry<'_>, profile: HistogramProfile) -> String {
    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf).histogram_profile(profile);
        registry.encode(&mut enc).unwrap();
        enc.finish().unwrap();
    }
    buf
}

fn render(schema: &MetricSchema, values: &MetricValues) -> String {
    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        enc.encode_document(schema, values).unwrap();
        enc.finish().unwrap();
    }
    buf
}

#[test]
fn unknown_type_renders_type_line_and_plain_sample() {
    let mut schema = MetricSchema::new();
    schema.add_family(
        "passthrough_thing",
        MetricType::Unknown,
        &[("src", "legacy")],
    );
    let mut values = MetricValues::new();
    values.sample("passthrough_thing", &[("src", "legacy")], 42u64);

    let text = render(&schema, &values);
    assert!(text.contains("# TYPE passthrough_thing unknown"));
    assert!(text.contains("passthrough_thing{src=\"legacy\"} 42"));
}

#[test]
fn gauge_histogram_renders_gaugehistogram_type() {
    use metered::gauge_histogram::{GaugeBuckets, GaugeHistogram};
    use metered::Metric;

    let hist = GaugeBuckets::new([1.0, 2.0]);
    hist.enter(0.5);
    hist.enter(1.5);
    let gh = GaugeHistogram::new(hist);

    let mut schema = MetricSchema::new();
    gh.describe_metric("queue_size", &[], &mut schema);
    let mut values = MetricValues::new();
    gh.collect_metric("queue_size", &[], &mut values);

    let text = render(&schema, &values);
    assert!(text.contains("# TYPE queue_size gaugehistogram"));
    assert!(text.contains("queue_size_gcount 2"));
    assert!(text.contains("queue_size_gsum 2"));
    assert!(text.contains("queue_size_bucket{le=\"+Inf\"} 2"));
}

#[test]
fn registry_registers_direct_tree_source_entry() {
    #[derive(Default, MetricTree)]
    struct ChildMetrics {
        #[metric(counter)]
        jobs: AtomicU64,
    }

    let child = ChildMetrics::default();
    child.jobs.incr_by(4);

    // Mounted under the "child" name segment below, so the view carries no
    // prefix (the mount segment names it; no hidden prefix-equals-name dedup).
    let mut child_view = metered::MetricTreeView::new();
    child_view.register(metric("metrics").select(|child: &ChildMetrics| child));

    let mut registry = Registry::with_prefix("app");
    registry.register(tree("child").source(&child).view(child_view));

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# TYPE app_child_metrics_jobs counter"));
    assert!(text.contains("app_child_metrics_jobs_total 4"));
}

#[test]
fn histogram_render_resolves_family_intent_against_document_capability() {
    let fixed = FixedExponentialHistogram::new(0.001, 10.0, 4);
    let dynamic = DynamicExponentialHistogram::with_params(5, 256);
    for v in [0.002, 0.05, 0.4, 3.0] {
        fixed.observe(v);
        dynamic.observe(v);
    }

    let mut registry = Registry::new();
    registry.register(
        metric("fixed_seconds")
            .source(&fixed)
            .help("Fixed exponential"),
    );
    registry.register(
        metric("dynamic_seconds")
            .source(&dynamic)
            .help("Dynamic exponential"),
    );

    // `Le` document capability: the scraper cannot ingest vmrange, so every
    // declaration degrades to classic cumulative `le` buckets.
    let le = encode(&registry, HistogramProfile::Le);
    assert!(le.contains("# TYPE fixed_seconds histogram"));
    assert!(le.contains("fixed_seconds_bucket{le="));
    assert!(le.contains("fixed_seconds_count 4"));
    assert!(le.contains("dynamic_seconds_bucket{le="));
    assert!(!le.contains("vmrange"));

    // vmrange-capable document: each family renders its *declared* form.
    // The fixed layout is shared by all series, so it keeps the classic
    // cross-service `le` contract; the dynamic backend rescales per series,
    // so it renders the aggregation-sound `vmrange` form.
    let vm = encode(&registry, HistogramProfile::VmRange);
    assert!(vm.contains("# TYPE fixed_seconds histogram"));
    assert!(vm.contains("fixed_seconds_bucket{le="));
    assert!(!vm.contains("fixed_seconds_bucket{vmrange=\""));
    assert!(vm.contains("dynamic_seconds_bucket{vmrange=\""));
    assert!(!vm.contains("dynamic_seconds_bucket{le="));
    assert!(vm.contains("fixed_seconds_count 4"));
    assert!(vm.contains("dynamic_seconds_count 4"));

    // The non-cumulative vmrange bucket counts sum to the total.
    let vmrange_total: u64 = vm
        .lines()
        .filter(|l| l.starts_with("dynamic_seconds_bucket{vmrange="))
        .filter_map(|l| l.rsplit(' ').next())
        .filter_map(|n| n.parse::<u64>().ok())
        .sum();
    assert_eq!(vmrange_total, 4);
}

#[test]
fn classic_bucket_histogram_stays_le_even_in_vmrange_profile() {
    let h = BucketHistogram::default();
    h.observe(0.012);
    h.observe(0.4);
    let mut registry = Registry::new();
    registry.register(
        metric("classic_seconds")
            .source(&h)
            .help("Classic bucket histogram"),
    );

    // A classic bucket histogram has no native vmrange form, so it falls back
    // to `le` even when the encoder is in vmrange mode.
    let vm = encode(&registry, HistogramProfile::VmRange);
    assert!(vm.contains("classic_seconds_bucket{le="));
    assert!(!vm.contains("vmrange"));
}

#[test]
fn adapter_exposes_existing_atomics_without_duplicating_state() {
    // State the application already owns and mutates directly.
    let enabled = AtomicBool::new(false);
    let processed = AtomicU64::new(41);

    let enabled_metric = flag(|| enabled.load(Ordering::Relaxed));
    let processed_metric = CounterFn(|| processed.load(Ordering::Relaxed));

    // The metric reflects the live value at encode time.
    enabled.store(true, Ordering::Relaxed);
    processed.fetch_add(1, Ordering::Relaxed);

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        enabled_metric
            .encode("feature_enabled", &[("svc", "x")], &mut enc)
            .unwrap();
        processed_metric
            .encode("items_processed", &[("svc", "x")], &mut enc)
            .unwrap();
        enc.finish().unwrap();
    }

    assert!(buf.contains("# TYPE feature_enabled gauge"));
    assert!(buf.contains("feature_enabled{svc=\"x\"} 1"));
    assert!(buf.contains("# TYPE items_processed counter"));
    assert!(buf.contains("items_processed_total{svc=\"x\"} 42"));
}

#[test]
fn custom_struct_exposes_native_metrics_from_its_state() {
    // A domain type with its own state, exposed as native metrics by
    // implementing MetricTree.
    struct Queue {
        depth: AtomicU64,
    }
    impl MetricTree for Queue {
        fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
            schema.add_family(name, MetricType::Gauge, labels);
        }

        fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
            values.gauge(name, labels, self.depth.load(Ordering::Relaxed) as i64);
        }
    }

    let q = Queue {
        depth: AtomicU64::new(3),
    };
    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        q.encode("queue_depth", &[], &mut enc).unwrap();
        enc.finish().unwrap();
    }
    assert!(buf.contains("queue_depth 3"));
}

#[test]
fn family_of_counters_encodes_one_type_and_many_series() {
    let family: Family<Vec<(String, String)>, AtomicU64> = Family::with_label_names(["method"]);
    let get = vec![("method".to_owned(), "get".to_owned())];
    let post = vec![("method".to_owned(), "post".to_owned())];
    family.with(&get, metered::Counter::incr);
    family.with(&get, metered::Counter::incr);
    family.with(&post, metered::Counter::incr);

    assert_eq!(family.len(), 2);

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        family
            .encode("requests", &[("svc", "x")], &mut enc)
            .unwrap();
        enc.finish().unwrap();
    }
    assert_eq!(buf.matches("# TYPE requests counter").count(), 1);
    assert!(buf.contains("requests_total{svc=\"x\",method=\"get\"} 2"));
    assert!(buf.contains("requests_total{svc=\"x\",method=\"post\"} 1"));
}

#[derive(Default)]
struct Pool {
    acquired: AtomicU64,
    idle: AtomicU64,
}

impl MetricTree for Pool {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.acquired
            .describe(&join_name(name, "acquired"), labels, schema);
        self.idle.describe(&join_name(name, "idle"), labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.acquired
            .collect(&join_name(name, "acquired"), labels, values);
        self.idle.collect(&join_name(name, "idle"), labels, values);
    }
}

#[test]
fn renamed_changes_the_segment() {
    let hits = AtomicU64::new(0);
    metered::Counter::incr_by(&hits, 4);

    let renamed = Renamed::new("requests", &hits);
    let mut registry = Registry::new();
    registry.register(metric("api").source(&renamed).help("API requests"));

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# TYPE api_requests counter"));
    assert!(text.contains("api_requests_total 4"));
    assert!(!text.contains("api_total"));
}

#[test]
fn flatten_drops_the_segment_so_children_sit_at_the_parent() {
    let pool = Pool::default();
    metered::Counter::incr(&pool.acquired);
    metered::Counter::incr_by(&pool.idle, 2);

    // Without flattening, `register("db", ..., &pool)` would emit
    // `db_acquired_total` / `db_idle_total`. Flattening keeps them at `db`.
    let flat = Flatten::new(&pool);
    let mut registry = Registry::new();
    registry.register(metric("db").source(&flat).help("DB pool"));

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("db_acquired_total 1"));
    assert!(text.contains("db_idle_total 2"));
}

#[test]
fn registry_applies_prefix_labels_help_and_unit() {
    let requests = AtomicU64::new(0);
    metered::Counter::incr(&requests);
    let depth = AtomicI64::new(0);
    metered::Gauge::set(&depth, 2);

    let mut registry = Registry::with_prefix("app");
    registry.label("env", "test");
    registry.register(
        metric("requests")
            .source(&requests)
            .help("Requests handled"),
    );
    registry.register(
        metric("queue_depth")
            .source(&depth)
            .help("Queue depth")
            .unit("items"),
    );

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# HELP app_requests Requests handled"));
    assert!(text.contains("# TYPE app_requests counter"));
    assert!(text.contains("app_requests_total{env=\"test\"} 1"));
    assert!(text.contains("# TYPE app_queue_depth gauge"));
    // `items` is not an `_`-separated suffix of `app_queue_depth`, so the
    // non-conformant `# UNIT` line is suppressed (it would make Prometheus
    // reject the whole scrape); the family itself still renders.
    assert!(!text.contains("# UNIT app_queue_depth"));
    assert!(text.contains("app_queue_depth{env=\"test\"} 2"));
    assert!(text.trim_end().ends_with("# EOF"));
}

#[test]
fn registry_accepts_typed_name_help_and_unit() {
    use metered::{AsGauge, Help, Name, Unit};

    let depth = AtomicU64::new(7);
    let depth_metric = AsGauge::from(&depth);
    let mut registry = Registry::with_prefix(Name::from("app"));
    registry.register(
        metric(Name::from("queue_depth"))
            .source(&depth_metric)
            .help(Help::from("Queue depth"))
            .unit(Unit::Items),
    );

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# HELP app_queue_depth Queue depth"));
    // `items` is not an `_`-separated suffix of `app_queue_depth`, so the
    // non-conformant `# UNIT` line is suppressed rather than risk Prometheus
    // rejecting the scrape.
    assert!(!text.contains("# UNIT app_queue_depth"));
    assert!(text.contains("app_queue_depth 7"));
}

#[test]
fn registry_register_uses_self_describing_entries() {
    use metered::entry::{counter, gauge};
    use metered::{Registry, Unit};
    use metered_om::OpenMetricsRegistryExt;
    use std::sync::atomic::AtomicU64;

    let requests = AtomicU64::new(5);
    let queue_depth = AtomicU64::new(9);

    let mut registry = Registry::with_prefix("app");
    registry.register(
        counter("requests")
            .source(&requests)
            .help("Total requests")
            .unit(Unit::Requests),
    );
    registry.register(
        gauge("queue_depth")
            .source(&queue_depth)
            .help("Queue depth")
            .unit(Unit::Items),
    );

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# TYPE app_requests counter"));
    assert!(text.contains("# UNIT app_requests requests"));
    assert!(text.contains("app_requests_total 5"));
    assert!(text.contains("# TYPE app_queue_depth gauge"));
    // Non-conformant unit (`items` is not a suffix of `app_queue_depth`) is
    // dropped; the conformant `app_requests` + `requests` above still emits.
    assert!(!text.contains("# UNIT app_queue_depth"));
    assert!(text.contains("app_queue_depth 9"));
}

#[test]
fn registry_composes_a_family() {
    let by_method: Family<Vec<(String, String)>, AtomicU64> = Family::with_label_names(["method"]);
    by_method.with(&vec![("method".to_owned(), "get".to_owned())], |c| {
        metered::Counter::incr(c)
    });

    let mut registry = Registry::new();
    registry.register(
        metric("requests")
            .source(&by_method)
            .help("Requests by method"),
    );

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# HELP requests Requests by method"));
    assert!(text.contains("# TYPE requests counter"));
    assert!(text.contains("requests_total{method=\"get\"} 1"));
}

#[test]
fn registry_metadata_only_applies_to_the_exact_registered_family() {
    #[derive(Default)]
    struct Ops {
        hits: AtomicU64,
    }

    impl MetricTree for Ops {
        fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
            self.hits.describe(&join_name(name, "hits"), labels, schema);
        }

        fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
            self.hits.collect(&join_name(name, "hits"), labels, values);
        }
    }

    let ops = Ops::default();
    metered::Counter::incr(&ops.hits);

    let mut registry = Registry::new();
    registry.register(metric("ops").source(&ops).help("Composite ops registry"));
    registry.register(metric("plain").source(&ops.hits).help("Plain leaf counter"));

    let text = registry.encode_to_string().unwrap();
    assert!(!text.contains("# HELP ops_hits Composite ops registry"));
    assert!(!text.contains("# HELP plain Composite ops registry"));
    assert!(text.contains("# HELP plain Plain leaf counter"));
    assert!(text.contains("# TYPE ops_hits counter"));
    assert!(text.contains("# TYPE plain counter"));
}

#[test]
fn registry_can_register_encode_only_metrics_with_explicit_schema() {
    let mut registry = Registry::with_prefix("app");
    registry.label("service", "api");
    registry.register_opaque(
        "external_depth",
        "Depth read from an external type",
        MetricType::Gauge,
        ["queue"],
        |name, labels, values| values.gauge(name, labels, 7),
    );
    registry.register_opaque_with_unit(
        "external_latency_seconds",
        "Latency read from an external type",
        "seconds",
        MetricType::Gauge,
        ["queue"],
        |name, labels, values| values.gauge(name, labels, 7),
    );

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# HELP app_external_depth Depth read from an external type"));
    assert!(text.contains("app_external_depth{service=\"api\"} 7"));
    assert!(text.contains("# UNIT app_external_latency_seconds seconds"));

    let schema = registry.schema();
    let family = schema.family("app_external_depth").unwrap();
    assert_eq!(family.metric_type, MetricType::Gauge);
    assert_eq!(family.labels, vec!["queue", "service"]);
    let latency = schema.family("app_external_latency_seconds").unwrap();
    assert_eq!(
        latency.unit.as_ref().map(metered::Unit::as_str),
        Some("seconds")
    );

    let values = registry.values();
    assert!(values
        .samples()
        .iter()
        .any(|sample| sample.name == "app_external_depth" && sample.value.to_string() == "7"));
}

/// The direct `MetricTree::encode` scrape path -- used by
/// `OpenMetricsExt::encode_to_string`, e.g. the order-service demo's
/// `app.encode_to_string()` -- must run scrape-time maintenance the way
/// `Registry::values()` does. A `DynamicExponentialHistogram` reopens its
/// sampled-exemplar window only inside `housekeep`; if `encode` never calls it,
/// the window stays `TAKEN` after the first adoption and the exemplar freezes
/// for the life of the process.
///
/// Proof: adopt exemplar `"a"`, scrape, then observe `"b"` into the *same*
/// bucket and scrape again. A fresh window must surface `"b"`; if the encode
/// path never housekeeps, the second scrape still shows the frozen `"a"`.
#[test]
fn metric_tree_encode_reopens_exemplar_window_each_scrape() {
    use metered::Exemplar;
    use metered_om::OpenMetricsExt;

    #[derive(Default, MetricTree)]
    #[metrics(prefix = "demo")]
    struct Tree {
        #[metric]
        latency: DynamicExponentialHistogram,
    }

    fn exemplar(trace: &str) -> Exemplar {
        Exemplar {
            labels: vec![("trace".to_owned(), trace.to_owned())],
            value: 0.5,
            timestamp_seconds: None,
        }
    }

    let tree = Tree::default();

    tree.latency
        .observe_with_exemplar(0.5, exemplar("a"), false);
    let scrape1 = tree.encode_to_string().unwrap();
    assert!(
        scrape1.contains("{trace=\"a\"}"),
        "first scrape should carry the first-window exemplar:\n{scrape1}"
    );

    // Same bucket (0.5), in what should be a new scrape window. It can only be
    // adopted if the previous `encode_to_string` reopened the window.
    tree.latency
        .observe_with_exemplar(0.5, exemplar("b"), false);
    let scrape2 = tree.encode_to_string().unwrap();
    assert!(
        scrape2.contains("{trace=\"b\"}"),
        "second scrape still shows the frozen first-window exemplar -- the \
         MetricTree::encode path never reopened the window via housekeep:\n{scrape2}"
    );
}
