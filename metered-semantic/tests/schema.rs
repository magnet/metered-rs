use metered::{BucketHistogram, Family, Help, MetricTree, MetricTreeMeta, Registry, Unit};
use metered_om::{OpenMetricsDocument, OpenMetricsEncoder, OpenMetricsRegistryExt};
use metered_semantic::{Elapsed, HitCount};
use std::sync::atomic::{AtomicI64, AtomicU64};

#[metered_semantic::error_count(name = InnerCount, visibility = pub)]
pub enum InnerError {
    Read,
    Write,
}

#[metered_semantic::error_count(name = OuterCount, visibility = pub)]
pub enum OuterError {
    Inner(#[nested] InnerError),
    Timeout,
}

#[derive(Default, MetricTree)]
#[metrics(
    prefix = "catalog_internal",
    help = "Catalog internal metrics",
    unit = "items"
)]
struct CatalogMetricsWithMeta {
    #[metric(counter)]
    cache_entries: AtomicU64,
}

#[derive(Default, MetricTree)]
#[metrics(prefix = "catalog_internal")]
struct CatalogMetricsWithoutHelpOrUnit {
    #[metric(counter)]
    cache_entries: AtomicU64,
}

#[test]
fn derive_metric_tree_emits_metadata_defaults() {
    assert_eq!(
        CatalogMetricsWithMeta::help().as_ref().map(Help::as_str),
        Some("Catalog internal metrics")
    );
    assert_eq!(
        CatalogMetricsWithMeta::unit().as_ref().map(Unit::as_str),
        Some("items")
    );
    assert_eq!(CatalogMetricsWithoutHelpOrUnit::help(), None);
    assert_eq!(CatalogMetricsWithoutHelpOrUnit::unit(), None);
}

#[derive(Default, MetricTree)]
struct JobRunnerMetrics {
    // `AtomicU64` defaults to a counter; `gauge` flips it to a gauge.
    #[metric(gauge, help = "Queued jobs waiting for the runner")]
    queue_depth: AtomicU64,
    #[metric(counter, help = "Runner run attempts")]
    runs: AtomicU64,
    #[metric(counter, help = "Runner run failures")]
    failures: AtomicU64,
    #[metric(unit = "seconds", help = "Runner run duration")]
    run_duration_seconds: BucketHistogram,
}

#[test]
fn derive_metric_tree_applies_per_field_kind_help_and_unit() {
    let metrics = JobRunnerMetrics::default();
    metered::Gauge::set(&metrics.queue_depth, 3);
    metered::Counter::incr(&metrics.runs);

    let mut registry = Registry::with_prefix("worker");
    registry.register(
        metered::entry::metric("jobs")
            .source(&metrics)
            .help("Job runner metrics"),
    );

    let schema = registry.schema();

    let queue = schema.family("worker_jobs_queue_depth").unwrap();
    assert_eq!(queue.metric_type, metered::MetricType::Gauge);
    assert_eq!(
        queue.help.as_ref().map(Help::as_str),
        Some("Queued jobs waiting for the runner")
    );

    let runs = schema.family("worker_jobs_runs").unwrap();
    assert_eq!(runs.metric_type, metered::MetricType::Counter);
    assert_eq!(
        runs.help.as_ref().map(Help::as_str),
        Some("Runner run attempts")
    );

    let failures = schema.family("worker_jobs_failures").unwrap();
    assert_eq!(failures.metric_type, metered::MetricType::Counter);

    let duration = schema.family("worker_jobs_run_duration_seconds").unwrap();
    assert_eq!(duration.metric_type, metered::MetricType::Histogram);
    assert_eq!(duration.unit.as_ref().map(Unit::as_str), Some("seconds"));

    // The gauge override renders as a gauge (no `_total`), the counters keep it.
    let text = registry.encode_to_string().unwrap();
    let doc = OpenMetricsDocument::parse(&text).unwrap();
    assert_eq!(doc.sample("worker_jobs_queue_depth").unwrap().value, "3");
    assert_eq!(doc.sample("worker_jobs_runs_total").unwrap().value, "1");
}

#[test]
fn registry_schema_describes_registered_metric_families() {
    let requests = AtomicU64::new(0);
    let depth = AtomicI64::new(0);
    let by_route: Family<Vec<(String, String)>, AtomicU64> = Family::with_label_names(["route"]);

    let mut registry = Registry::with_prefix("demo");
    registry.label("service", "metered-demo");
    registry.register(
        metered::entry::metric("requests")
            .source(&requests)
            .help("Total requests handled"),
    );
    registry.register(
        metered::entry::metric("queue_depth")
            .source(&depth)
            .help("Queue depth")
            .unit("items"),
    );
    registry.register(
        metered::entry::metric("by_route")
            .source(&by_route)
            .help("Requests by route"),
    );

    let schema = registry.schema();

    let requests = schema.family("demo_requests").expect("requests family");
    assert_eq!(requests.metric_type, metered::MetricType::Counter);
    assert_eq!(
        requests.help.as_ref().map(metered::Help::as_str),
        Some("Total requests handled")
    );
    assert_eq!(requests.unit.as_ref().map(metered::Unit::as_str), None);
    assert_eq!(requests.labels, vec!["service"]);

    let depth = schema
        .family("demo_queue_depth")
        .expect("queue depth family");
    assert_eq!(depth.metric_type, metered::MetricType::Gauge);
    assert_eq!(
        depth.help.as_ref().map(metered::Help::as_str),
        Some("Queue depth")
    );
    assert_eq!(
        depth.unit.as_ref().map(metered::Unit::as_str),
        Some("items")
    );
    assert_eq!(depth.labels, vec!["service"]);

    let route = schema.family("demo_by_route").expect("route family");
    assert_eq!(route.metric_type, metered::MetricType::Counter);
    assert_eq!(
        route.help.as_ref().map(metered::Help::as_str),
        Some("Requests by route")
    );
    assert_eq!(route.labels, vec!["route", "service"]);
}

#[test]
fn family_schema_does_not_require_observed_series() {
    let by_route: Family<Vec<(String, String)>, AtomicU64> = Family::with_label_names(["route"]);

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("requests")
            .source(&by_route)
            .help("Requests by route"),
    );

    let family = registry
        .schema()
        .family("requests")
        .expect("family should be described even before any series exists")
        .clone();
    assert_eq!(family.metric_type, metered::MetricType::Counter);
    assert_eq!(family.labels, vec!["route"]);
}

#[test]
fn nested_error_breakdown_schema_matches_encoded_families() {
    let errors = OuterCount::default();

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("errors")
            .source(&errors)
            .help("Errors by kind"),
    );

    let schema = registry.schema();
    let root = schema.family("errors").expect("flat outer error family");
    assert_eq!(root.labels, vec!["error_kind"]);

    let nested = schema
        .family("errors_inner")
        .expect("nested inner error family");
    assert_eq!(nested.metric_type, metered::MetricType::Counter);
    assert_eq!(nested.labels, vec!["error_kind"]);
}

#[derive(Default)]
struct Worker {
    metrics: WorkerMetrics,
}

#[metered_semantic::metered(registry = WorkerMetrics)]
impl Worker {
    #[measure([HitCount, Elapsed])]
    fn run(&self) {}
}

#[test]
fn generated_metered_registries_describe_their_child_families() {
    let worker = Worker::default();
    let mut registry = Registry::with_prefix("demo");
    registry.register(
        metered::entry::metric("worker")
            .source(&worker.metrics)
            .help("Worker metrics"),
    );

    let schema = registry.schema();

    assert!(schema.family("demo_worker").is_none());

    let hits = schema
        .family("demo_worker_run_hit_count")
        .expect("generated hit count family");
    assert_eq!(hits.metric_type, metered::MetricType::Counter);
    assert_eq!(hits.help, None);

    let elapsed = schema
        .family("demo_worker_run_elapsed")
        .expect("generated elapsed family");
    assert_eq!(elapsed.metric_type, metered::MetricType::Histogram);
}

#[test]
fn schema_generates_promql_dashboard_queries() {
    let latency: Elapsed = Elapsed::default();
    let requests = AtomicU64::new(0);

    let mut registry = Registry::with_prefix("demo");
    registry.register(
        metered::entry::metric("requests")
            .source(&requests)
            .help("Total requests handled"),
    );
    registry.register(
        metered::entry::metric("operation_duration_seconds")
            .source(&latency)
            .help("Operation latency")
            .unit("seconds"),
    );

    let queries = registry.schema().queries(metered::QueryDialect::PromQl);

    assert!(queries.iter().any(|query| {
        query.title == "demo_requests rate"
            && query.expr == "sum(rate(demo_requests_total[$__rate_interval]))"
    }));
    assert!(queries.iter().any(|query| {
        query.title == "demo_operation_duration_seconds p95"
            && query.expr
                == "histogram_quantile(0.95, sum(rate(demo_operation_duration_seconds_bucket[$__rate_interval])) by (le))"
    }));
}

#[test]
fn custom_leaf_metric_defines_type_and_encoding_once() {
    struct StaticGauge;

    impl metered::Metric for StaticGauge {
        fn metric_type(&self) -> metered::MetricType {
            metered::MetricType::Gauge
        }

        fn collect_metric(
            &self,
            name: &str,
            labels: &[(&str, &str)],
            values: &mut metered::MetricValues,
        ) {
            values.sample(name, labels, 7);
        }
    }

    let gauge = StaticGauge;
    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("custom")
            .source(&gauge)
            .help("Custom leaf gauge"),
    );

    let text = registry.encode_to_string().unwrap();
    let parsed = OpenMetricsDocument::parse(&text).unwrap();
    assert_eq!(
        parsed.family("custom").unwrap().metric_type,
        Some(metered::MetricType::Gauge)
    );
    assert_eq!(parsed.sample("custom").unwrap().value, "7");

    let schema = registry.schema();
    assert_eq!(
        schema.family("custom").unwrap().metric_type,
        metered::MetricType::Gauge
    );
}

#[test]
fn schema_and_values_render_to_openmetrics_document() {
    let requests = AtomicU64::new(0);
    metered::Counter::incr(&requests);
    let queue_depth = AtomicI64::new(0);
    metered::Gauge::set(&queue_depth, 9);

    let mut registry = Registry::with_prefix("demo");
    registry.label("service", "api");
    registry.register(
        metered::entry::metric("requests")
            .source(&requests)
            .help("Total requests"),
    );
    registry.register(
        metered::entry::metric("queue_depth")
            .source(&queue_depth)
            .help("Queue depth")
            .unit("items"),
    );

    let schema = registry.schema();
    let values = registry.values();

    let mut text = String::new();
    {
        let mut encoder = OpenMetricsEncoder::new(&mut text);
        encoder.encode_document(&schema, &values).unwrap();
        encoder.finish().unwrap();
    }

    let parsed = OpenMetricsDocument::parse(&text).unwrap();
    assert_eq!(
        parsed.family("demo_requests").unwrap().help.as_deref(),
        Some("Total requests")
    );
    // `items` is not an `_`-separated suffix of `demo_queue_depth`, so the
    // non-conformant `# UNIT` line is suppressed (emitting it would make
    // Prometheus reject the whole scrape); the family itself still renders.
    assert_eq!(
        parsed.family("demo_queue_depth").unwrap().unit.as_deref(),
        None
    );
    assert_eq!(
        parsed
            .sample("demo_requests_total")
            .unwrap()
            .label("service"),
        Some("api")
    );
    assert_eq!(parsed.sample("demo_queue_depth").unwrap().value, "9");
}
