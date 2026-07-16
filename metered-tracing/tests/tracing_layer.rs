use metered::bucket_histogram::{ExemplarSource, ThreadLocalExemplars};
use metered::{Buckets, Exemplar, Registry};
use metered_om::OpenMetricsRegistryExt;
use metered_semantic::{Elapsed, ElapsedConfig};
use metered_tracing::{FieldExemplarProvider, SpanMetric, TracingExemplarLayer, TracingMetrics};
use std::sync::{Arc, Mutex};
use tracing_subscriber::prelude::*;

/// Records trace ids observed via the ambient exemplar context.
#[derive(Clone, Default)]
struct RecordedTraceIds(Arc<Mutex<Vec<String>>>);

impl ExemplarSource for RecordedTraceIds {
    fn exemplar(&self) -> Option<Exemplar> {
        let exemplar = ThreadLocalExemplars.exemplar()?;
        let trace_id = exemplar
            .labels
            .iter()
            .find(|(name, _)| name == "trace_id")
            .map(|(_, value)| value.clone())?;
        self.0.lock().unwrap().push(trace_id);
        Some(exemplar)
    }
}

fn render_span_metric(name: &'static str, metric: &SpanMetric) -> String {
    let mut registry = Registry::new();
    registry.register(metered::entry::metric(name).source(metric));
    registry.encode_to_string().expect("render span metric")
}

fn sample_value<'a>(text: &'a str, metric: &str) -> Option<&'a str> {
    text.lines()
        .find(|line| line.starts_with(metric))
        .map(|line| line.rsplit_once(' ').map(|(_, v)| v).unwrap_or(line))
}

#[test]
fn span_close_records_semantic_family_labeled_from_fields() {
    let rpc = SpanMetric::for_span("rpc.server")
        .help("RPC server calls")
        .label("rpc_method", "rpc.method")
        .label("rpc_status", "rpc.grpc.status_code")
        .build();
    let layer = TracingMetrics::builder().recorder(rpc.clone()).build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "rpc.server",
            rpc.method = "CreateOrder",
            "rpc.grpc.status_code" = "OK"
        );
        let _entered = span.enter();
    });

    let text = render_span_metric("rpc_server", &rpc);

    assert!(text.contains("# TYPE rpc_server_requests counter"));
    assert!(text.contains("# HELP rpc_server_requests RPC server calls"));
    assert!(text.contains("# TYPE rpc_server_duration_seconds histogram"));
    assert!(text.contains("# UNIT rpc_server_duration_seconds seconds"));
    assert_eq!(
        sample_value(
            &text,
            "rpc_server_requests_total{rpc_method=\"CreateOrder\",rpc_status=\"OK\"}"
        ),
        Some("1")
    );
    assert_eq!(
        sample_value(
            &text,
            "rpc_server_duration_seconds_count{rpc_method=\"CreateOrder\",rpc_status=\"OK\"}"
        ),
        Some("1")
    );
}

#[test]
fn span_metric_duration_captures_exemplar_from_trace_fields_at_close() {
    let rpc = SpanMetric::for_span("rpc.server")
        .label("rpc_method", "rpc.method")
        .build();
    let layer = TracingMetrics::builder()
        .recorder(rpc.clone())
        .build()
        .with_exemplar_provider(FieldExemplarProvider::new(["trace_id", "span_id"]));
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!(
            "rpc.server",
            rpc.method = "CreateOrder",
            trace_id = "4bf92f3577b34da6a3ce929d0e0e4736",
            span_id = "00f067aa0ba902b7"
        )
        .in_scope(|| {});
    });

    let text = render_span_metric("rpc_server", &rpc);
    // The duration histogram carries an exemplar on the bucket the value landed in.
    assert!(
        text.lines().any(|line| {
            line.contains("rpc_server_duration_seconds_bucket")
            && line.contains(
                "# {trace_id=\"4bf92f3577b34da6a3ce929d0e0e4736\",span_id=\"00f067aa0ba902b7\"}"
            )
        }),
        "expected a trace exemplar on a duration bucket, got:\n{text}"
    );
}

#[test]
fn exemplars_work_without_distributed_tracing_using_a_local_id() {
    // No trace context: a local `order_id` is enough to point a latency bucket at
    // the exact entity behind it.
    let orders = SpanMetric::for_span("orders.create_order").build();
    let layer = TracingMetrics::builder()
        .recorder(orders.clone())
        .build()
        .with_exemplar_provider(FieldExemplarProvider::new(["order.id"]));
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        // The span attribute follows semconv (`order.id`); the exemplar label is
        // sanitized to a valid OpenMetrics name (`order_id`).
        tracing::info_span!("orders.create_order", order.id = 4242u64).in_scope(|| {});
    });

    let text = render_span_metric("orders_create", &orders);
    assert!(
        text.lines().any(
            |line| line.contains("orders_create_duration_seconds_bucket")
                && line.contains("# {order_id=\"4242\"}")
        ),
        "expected a sanitized local order_id exemplar, got:\n{text}"
    );
}

#[test]
fn duplicate_span_names_dispatch_to_every_registered_recorder() {
    // Two profiles claim "rpc.server": both observe each close (an explicit
    // fan-out, e.g. a shadow rollout), rather than the later registration being
    // silently dropped.
    let first = SpanMetric::for_span("rpc.server")
        .label("rpc_method", "rpc.method")
        .build();
    let second = SpanMetric::for_span("rpc.server")
        .label("rpc_method", "rpc.method")
        .build();
    let bundle = TracingMetrics::builder()
        .recorder(first.clone())
        .recorder(second.clone())
        .build();
    let subscriber = tracing_subscriber::registry().with(bundle);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!("rpc.server", rpc.method = "CreateOrder").in_scope(|| {});
    });

    let primary = render_span_metric("rpc_primary", &first);
    assert_eq!(
        sample_value(
            &primary,
            "rpc_primary_requests_total{rpc_method=\"CreateOrder\"}"
        ),
        Some("1")
    );

    let shadow = render_span_metric("rpc_shadow", &second);
    assert_eq!(
        sample_value(
            &shadow,
            "rpc_shadow_requests_total{rpc_method=\"CreateOrder\"}"
        ),
        Some("1")
    );
}

#[test]
fn distinct_span_names_feed_distinct_semantic_families() {
    let rpc = SpanMetric::for_span("rpc.server")
        .label("rpc_method", "rpc.method")
        .build();
    let db = SpanMetric::for_span("db.query")
        .label("db_operation", "db.operation")
        .build();
    let layer = TracingMetrics::builder()
        .recorder(rpc.clone())
        .recorder(db.clone())
        .build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!("rpc.server", rpc.method = "CreateOrder").in_scope(|| {});
        tracing::info_span!("db.query", db.operation = "insert_order").in_scope(|| {});
        // An unmatched span produces no metrics at all.
        tracing::info_span!("uninteresting").in_scope(|| {});
    });

    let rpc_text = render_span_metric("rpc_server", &rpc);
    assert_eq!(
        sample_value(
            &rpc_text,
            "rpc_server_requests_total{rpc_method=\"CreateOrder\"}"
        ),
        Some("1")
    );

    let db_text = render_span_metric("db_client", &db);
    assert_eq!(
        sample_value(
            &db_text,
            "db_client_requests_total{db_operation=\"insert_order\"}"
        ),
        Some("1")
    );
}

#[test]
fn field_recorded_after_creation_uses_final_value_at_close() {
    let rpc = SpanMetric::for_span("rpc.server")
        .label("rpc_status", "rpc.grpc.status_code")
        .build();
    let layer = TracingMetrics::builder().recorder(rpc.clone()).build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let span =
            tracing::info_span!("rpc.server", "rpc.grpc.status_code" = tracing::field::Empty);
        span.record("rpc.grpc.status_code", "INTERNAL");
        let _entered = span.enter();
    });

    let text = render_span_metric("rpc_server", &rpc);
    assert_eq!(
        sample_value(&text, "rpc_server_requests_total{rpc_status=\"INTERNAL\"}"),
        Some("1")
    );
}

#[test]
fn absent_field_falls_back_to_label_default() {
    let rpc = SpanMetric::for_span("rpc.server")
        .label_or("rpc_status", "rpc.grpc.status_code", "OK")
        .build();
    let layer = TracingMetrics::builder().recorder(rpc.clone()).build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!("rpc.server").in_scope(|| {});
    });

    let text = render_span_metric("rpc_server", &rpc);
    assert_eq!(
        sample_value(&text, "rpc_server_requests_total{rpc_status=\"OK\"}"),
        Some("1")
    );
}

#[test]
fn exemplar_layer_feeds_metered_thread_local_exemplars_from_span_fields() {
    let provider = FieldExemplarProvider::new(["trace_id", "span_id"]);
    let subscriber = tracing_subscriber::registry().with(TracingExemplarLayer::new(provider));
    let elapsed: Elapsed<ThreadLocalExemplars> = Elapsed::default();

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "http.request",
            trace_id = "4bf92f3577b34da6a3ce929d0e0e4736",
            span_id = "00f067aa0ba902b7"
        );
        let _entered = span.enter();
        metered_semantic::measure!(&elapsed, {});
    });

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("http_server_request_duration_seconds")
            .source(&elapsed)
            .help("HTTP server request duration"),
    );
    let text = registry.encode_to_string().expect("render histogram");

    assert!(text.contains(
        "# {trace_id=\"4bf92f3577b34da6a3ce929d0e0e4736\",span_id=\"00f067aa0ba902b7\"}"
    ));
}

#[test]
fn custom_duration_buckets() {
    let tiny = SpanMetric::for_span("tiny")
        .duration_buckets(metered::Buckets::linear(0.001, 0.002, 2))
        .build();
    let layer = TracingMetrics::builder().recorder(tiny.clone()).build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("tiny");
        let _entered = span.enter();
    });

    let text = render_span_metric("tiny", &tiny);

    assert!(text.contains("tiny_duration_seconds_bucket{"));
    assert!(text.contains("le=\"0.001\""));
    assert!(text.contains("le=\"0.002\""));
}

#[test]
fn combined_metrics_and_exemplar_layer_uses_one_span_extension() {
    let provider = FieldExemplarProvider::new(["trace_id", "span_id"]);
    let http = SpanMetric::for_span("http.request")
        .label("http_status", "otel.status_code")
        .build();
    let metrics = TracingMetrics::builder()
        .recorder(http.clone())
        .build()
        .with_exemplar_provider(provider);
    let subscriber = tracing_subscriber::registry().with(metrics);
    let elapsed: Elapsed<ThreadLocalExemplars> = Elapsed::default();

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "http.request",
            "otel.status_code" = "OK",
            trace_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            span_id = "bbbbbbbbbbbbbbbb"
        );
        let _entered = span.enter();
        metered_semantic::measure!(&elapsed, {});
    });

    let text = render_span_metric("http_server", &http);
    assert_eq!(
        sample_value(&text, "http_server_requests_total{http_status=\"OK\"}"),
        Some("1")
    );

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("request_duration_seconds")
            .source(&elapsed)
            .help("request duration"),
    );
    let elapsed_text = registry.encode_to_string().expect("render elapsed");
    assert!(elapsed_text.contains(
        "# {trace_id=\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",span_id=\"bbbbbbbbbbbbbbbb\"}"
    ));
}

/// The `SpanLabels`-derived opener is `#[macro_export]`ed, so `metered_info_span!`
/// resolves it from a different module than the one the struct is declared in
/// (and the call here is textually *before* the `inner` module). This is the
/// regression guard for the old "opener only works in the defining module, after
/// the struct" footgun.
#[test]
fn span_labels_opener_resolves_across_modules() {
    use metered::{entry::metric, DynamicExponentialHistogram, Family};
    use metered_tracing::{metered_info_span, SpanDurations};
    use std::sync::Arc;

    // The duration family lives inside a component; the adapter Arcs the
    // component and projects to the family it owns.
    struct Rpc {
        duration: Family<inner::CrossModuleLabels, DynamicExponentialHistogram>,
    }
    let rpc = Arc::new(Rpc {
        duration: Family::default(),
    });
    let layer = TracingMetrics::builder()
        .recorder(SpanDurations::on(
            inner::CrossModuleLabels::SPAN,
            &rpc,
            |rpc: &Rpc| &rpc.duration,
        ))
        .build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        // The struct lives in `inner`; the opener macro is at the crate root.
        let span = metered_info_span!(CrossModuleLabels; rpc_method = "CreateOrder".to_owned());
        span.in_scope(|| {});
    });

    let mut registry = Registry::new();
    registry.register(metric("rpc_server_duration_seconds").source(&rpc.duration));
    let text = registry.encode_to_string().expect("render duration");
    assert_eq!(
        sample_value(
            &text,
            "rpc_server_duration_seconds_count{rpc_method=\"CreateOrder\"}"
        ),
        Some("1")
    );
}

mod inner {
    #[derive(Clone, PartialEq, Eq, Hash, metered::LabelSet, metered_tracing::SpanLabels)]
    #[span(name = "rpc.server", help = "RPC server calls")]
    pub struct CrossModuleLabels {
        #[span("rpc.method")]
        pub rpc_method: String,
    }
}

#[derive(Clone, Hash, PartialEq, Eq, metered::LabelSet, metered_tracing::SpanLabels)]
#[span(name = "db.query", help = "DB query duration")]
struct DbLabels {
    #[span("db.operation.name")]
    db_operation: String,
}

#[test]
fn span_durations_projects_from_component_arc() {
    use metered::{DynamicExponentialHistogram, Family};
    use metered_tracing::{metered_info_span, SpanDurations};
    use std::sync::Arc;

    // Metrics are plain fields; the Arc wraps the component, not the metric.
    struct Db {
        duration: Family<DbLabels, DynamicExponentialHistogram>,
    }

    let db = Arc::new(Db {
        duration: Family::default(),
    });

    let telemetry = TracingMetrics::builder()
        .recorder(SpanDurations::on(DbLabels::SPAN, &db, |db: &Db| {
            &db.duration
        }))
        .build();

    tracing::subscriber::with_default(tracing_subscriber::registry().with(telemetry), || {
        metered_info_span!(DbLabels; db_operation = "insert".to_owned()).in_scope(|| {});
    });

    let mut values = metered::MetricValues::new();
    metered::MetricTree::collect(&db.duration, "db_duration", &[], &mut values);
    assert!(
        values.histograms().iter().any(|h| h.name == "db_duration"),
        "projected family should have recorded the span close"
    );
}

#[test]
fn adopted_exemplars_fire_the_retention_hook() {
    use metered::{DynamicExponentialHistogram, Family};
    use metered_tracing::{metered_info_span, SpanDurations};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // The duration family lives inside a component the adapter projects to; the
    // hook is the trace-retention seam, firing with the adopted exemplar (whose
    // labels carry the trace id).
    struct Holder {
        duration: Family<DbLabels, DynamicExponentialHistogram>,
    }
    let holder = Arc::new(Holder {
        duration: Family::default(),
    });

    let adopted = Arc::new(AtomicUsize::new(0));
    let adopted_in_hook = adopted.clone();

    let telemetry = TracingMetrics::builder()
        .recorder(SpanDurations::on(DbLabels::SPAN, &holder, |h: &Holder| {
            &h.duration
        }))
        .on_exemplar_adopted(move |exemplar| {
            assert!(
                exemplar.labels.iter().any(|(k, _)| k == "trace_id"),
                "adopted exemplar should carry the trace id"
            );
            adopted_in_hook.fetch_add(1, Ordering::Relaxed);
        })
        .build()
        .with_exemplar_provider(FieldExemplarProvider::new(["trace_id"]));
    let subscriber = tracing_subscriber::registry().with(telemetry);

    tracing::subscriber::with_default(subscriber, || {
        metered_info_span!(
            DbLabels;
            db_operation = "insert".to_owned(),
            trace_id = "4bf92f3577b34da6a3ce929d0e0e4736"
        )
        .in_scope(|| {});
    });

    assert_eq!(
        adopted.load(Ordering::Relaxed),
        1,
        "first window adopts exactly once"
    );
}

#[derive(Clone, Hash, PartialEq, Eq, metered::LabelSet, metered_tracing::SpanLabels)]
#[span(name = "queue.pop", help = "Queue pops by shard")]
struct ShardLabels {
    // A typed numeric label: captured natively (no string round-trip) and
    // fallible on conversion.
    #[span("shard.index")]
    shard: u64,
}

#[test]
fn malformed_span_fields_skip_the_observation_and_are_counted() {
    use metered::{DynamicExponentialHistogram, Family};
    use metered_tracing::SpanDurations;

    struct Queue {
        duration: Family<ShardLabels, DynamicExponentialHistogram>,
    }
    let queue = Arc::new(Queue {
        duration: Family::default(),
    });
    let telemetry = TracingMetrics::builder()
        .recorder(SpanDurations::on(ShardLabels::SPAN, &queue, |q: &Queue| {
            &q.duration
        }))
        .build();
    let malformed = telemetry.malformed_spans();
    let subscriber = tracing_subscriber::registry().with(telemetry);

    tracing::subscriber::with_default(subscriber, || {
        // A natively-typed u64 converts without any string round-trip.
        tracing::info_span!("queue.pop", shard.index = 7u64).in_scope(|| {});
        // A producer violating the contract (text where a u64 is declared):
        // the close is skipped and counted, not defaulted onto shard 0.
        tracing::info_span!("queue.pop", shard.index = "not-a-shard").in_scope(|| {});
    });

    let mut values = metered::MetricValues::new();
    metered::MetricTree::collect(&queue.duration, "queue_pop", &[], &mut values);
    let shard_series: Vec<_> = values
        .histograms()
        .iter()
        .flat_map(|h| h.labels.iter())
        .collect();
    assert_eq!(
        shard_series,
        vec![&("shard".to_owned(), "7".to_owned())],
        "only the well-formed close observed; nothing defaulted to shard 0"
    );
    assert_eq!(malformed.total(), 1, "the malformed close was counted");

    // The bounded error metric renders as a normal counter keyed by span name.
    let mut registry = Registry::new();
    registry.register(metered::entry::metric("span_labels_malformed").source(&malformed));
    let text = registry.encode_to_string().unwrap();
    assert!(
        text.contains("span_labels_malformed_total{span=\"queue.pop\"} 1"),
        "malformed counter renders under the span name label:\n{text}"
    );
}

#[test]
fn nested_spans_restore_parent_exemplar_on_exit() {
    let provider = FieldExemplarProvider::new(["trace_id", "span_id"]);
    let subscriber = tracing_subscriber::registry().with(TracingExemplarLayer::new(provider));
    let recorded = RecordedTraceIds::default();
    let elapsed = Elapsed::with_config(ElapsedConfig {
        buckets: Buckets::seconds_default(),
        exemplar_source: recorded.clone(),
    });

    tracing::subscriber::with_default(subscriber, || {
        let parent =
            tracing::info_span!("parent", trace_id = "parent-trace", span_id = "parent-span");
        let parent_entered = parent.enter();
        metered_semantic::measure!(&elapsed, {});

        {
            let child =
                tracing::info_span!("child", trace_id = "child-trace", span_id = "child-span");
            let _child_entered = child.enter();
            metered_semantic::measure!(&elapsed, {});
        }

        metered_semantic::measure!(&elapsed, {});
        drop(parent_entered);
    });

    let seen = recorded.0.lock().unwrap().clone();
    assert_eq!(
        seen,
        vec![
            "parent-trace".to_owned(),
            "child-trace".to_owned(),
            "parent-trace".to_owned(),
        ]
    );
}
