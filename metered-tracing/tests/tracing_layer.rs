use metered::bucket_histogram::{BucketHistogram, ExemplarSource, ThreadLocalExemplars};
use metered::{Buckets, Exemplar, Registry};
use metered_om::OpenMetricsRegistryExt;
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

/// Observes `value` on `histogram`, attaching whatever exemplar `source`
/// currently provides -- the caller-side half of the ambient exemplar seam,
/// built from plain metered primitives.
fn observe_with_source(histogram: &BucketHistogram, source: &impl ExemplarSource, value: f64) {
    match source.exemplar() {
        Some(mut exemplar) => {
            exemplar.value = value;
            histogram.observe_with_exemplar(value, exemplar);
        }
        None => {
            histogram.observe(value);
        }
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

/// The parsed value of a `_sum` sample, panicking when the sample is missing.
fn sum_value(text: &str, metric: &str) -> f64 {
    sample_value(text, metric)
        .unwrap_or_else(|| panic!("missing sample {metric}"))
        .parse()
        .expect("sum parses as f64")
}

/// A value whose `Debug` rendering panics: placed on a span field no
/// registered consumer reads, it proves the layer rejects the field by name
/// before any formatting or capture allocation happens.
struct PanicOnFormat;

impl std::fmt::Debug for PanicOnFormat {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("an unconsumed span field must never be formatted");
    }
}

/// Replaces the wall-clock `_sum` sample values with a placeholder so two
/// otherwise-identical scrapes of different spans compare byte-for-byte.
fn normalize_sums(text: &str) -> String {
    text.lines()
        .map(|line| match line.split_once(' ') {
            Some((series, _)) if series.contains("_sum") => format!("{series} <wall-clock>"),
            _ => line.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn unconsumed_span_fields_are_dropped_before_formatting_and_change_nothing() {
    let recorded = SpanMetric::for_span("rpc.server")
        .label("rpc_method", "rpc.method")
        .build();
    let control = SpanMetric::for_span("rpc.server")
        .label("rpc_method", "rpc.method")
        .build();

    let layer = TracingMetrics::builder().recorder(recorded.clone()).build();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        // `payload` is read by no recorder and no exemplar provider: the
        // capture visitor must drop it by name, so the panicking `Debug`
        // impl is never invoked and no owned String is built for it.
        tracing::info_span!(
            "rpc.server",
            rpc.method = "CreateOrder",
            payload = ?PanicOnFormat
        )
        .in_scope(|| {});
    });

    let layer = TracingMetrics::builder().recorder(control.clone()).build();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        tracing::info_span!("rpc.server", rpc.method = "CreateOrder").in_scope(|| {});
    });

    // The extra field changes nothing on the wire: byte-identical output,
    // modulo the two spans' wall-clock duration sums.
    assert_eq!(
        normalize_sums(&render_span_metric("rpc_server", &recorded)),
        normalize_sums(&render_span_metric("rpc_server", &control)),
        "a span field nobody reads must not change the rendered families"
    );
}

#[test]
fn exemplar_capture_drops_fields_the_provider_does_not_read() {
    // Field capture on spans no recorder matches is demanded only by the
    // exemplar provider, so it is filtered to the provider's declared fields.
    let provider = FieldExemplarProvider::new(["trace_id"]);
    let subscriber = tracing_subscriber::registry().with(TracingExemplarLayer::new(provider));
    let duration = BucketHistogram::new(Buckets::seconds_default());

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "http.request",
            trace_id = "4bf92f3577b34da6a3ce929d0e0e4736",
            payload = ?PanicOnFormat
        );
        let _entered = span.enter();
        observe_with_source(&duration, &ThreadLocalExemplars, 0.001);
    });

    let mut registry = Registry::new();
    registry.register(metered::entry::metric("request_duration_seconds").source(&duration));
    let text = registry.encode_to_string().expect("render histogram");
    assert!(
        text.contains("# {trace_id=\"4bf92f3577b34da6a3ce929d0e0e4736\"}"),
        "the demanded trace field still reaches the exemplar:\n{text}"
    );
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
        // Give the span a measurable lifetime so the duration sum assertion
        // below is deterministic rather than racing clock resolution.
        std::thread::sleep(std::time::Duration::from_millis(1));
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
    // The count alone would still pass if the close path observed 0.0 instead
    // of the span's wall-clock time; a positive sum pins the real measurement.
    assert!(
        sum_value(
            &text,
            "rpc_server_duration_seconds_sum{rpc_method=\"CreateOrder\",rpc_status=\"OK\"}"
        ) > 0.0,
        "span close should observe the span's wall-clock duration"
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
    let duration = BucketHistogram::new(Buckets::seconds_default());

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "http.request",
            trace_id = "4bf92f3577b34da6a3ce929d0e0e4736",
            span_id = "00f067aa0ba902b7"
        );
        let _entered = span.enter();
        observe_with_source(&duration, &ThreadLocalExemplars, 0.001);
    });

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("http_server_request_duration_seconds")
            .source(&duration)
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

    // `sleep` guarantees at least this much wall-clock span lifetime, past
    // both finite bucket bounds (1ms/2ms) -- so exactly which bucket the
    // close lands in is deterministic.
    let slept = std::time::Duration::from_millis(5);
    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!("tiny");
        let _entered = span.enter();
        std::thread::sleep(slept);
    });

    let text = render_span_metric("tiny", &tiny);

    // Bucket boundaries alone would pass without any recording; pin the close
    // that actually happened, in the exact bucket it must land in. A zeroed
    // duration would land in `le="0.001"` and fail both bucket assertions.
    assert_eq!(sample_value(&text, "tiny_requests_total"), Some("1"));
    assert_eq!(
        sample_value(&text, "tiny_duration_seconds_count"),
        Some("1")
    );
    for finite in ["0.001", "0.002"] {
        assert_eq!(
            sample_value(
                &text,
                &format!("tiny_duration_seconds_bucket{{le=\"{finite}\"}}")
            ),
            Some("0"),
            "a {}ms+ span must overflow the {finite}s bucket:\n{text}",
            slept.as_millis()
        );
    }
    assert_eq!(
        sample_value(&text, "tiny_duration_seconds_bucket{le=\"+Inf\"}"),
        Some("1"),
        "the recorded duration must land in the histogram"
    );
    // The sum is the span's real wall-clock lifetime: at least the sleep.
    assert!(
        sum_value(&text, "tiny_duration_seconds_sum") >= slept.as_secs_f64(),
        "duration sum must carry the span's wall-clock time:\n{text}"
    );
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
    let duration = BucketHistogram::new(Buckets::seconds_default());

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "http.request",
            "otel.status_code" = "OK",
            trace_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            span_id = "bbbbbbbbbbbbbbbb"
        );
        let _entered = span.enter();
        observe_with_source(&duration, &ThreadLocalExemplars, 0.001);
    });

    let text = render_span_metric("http_server", &http);
    assert_eq!(
        sample_value(&text, "http_server_requests_total{http_status=\"OK\"}"),
        Some("1")
    );

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("request_duration_seconds")
            .source(&duration)
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
    use metered::{DynamicExponentialHistogram, Family, entry::metric};
    use metered_tracing::{SpanDurations, metered_info_span};
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
        // A measurable lifetime keeps the duration sum assertion deterministic.
        span.in_scope(|| std::thread::sleep(std::time::Duration::from_millis(1)));
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
    assert!(
        sum_value(
            &text,
            "rpc_server_duration_seconds_sum{rpc_method=\"CreateOrder\"}"
        ) > 0.0,
        "span close should observe the span's wall-clock duration"
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
fn derived_label_set_lends_stored_strings_and_renders_unchanged() {
    use metered::LabelSet;

    let labels = DbLabels {
        db_operation: "insert".to_owned(),
    };
    let mut visited = Vec::new();
    labels.for_each_label(&mut |name, value| {
        assert_eq!(
            value.as_ptr(),
            labels.db_operation.as_ptr(),
            "a stored String label is lent to the visitor, not re-allocated \
             through ToString"
        );
        visited.push((name.to_owned(), value.to_owned()));
    });
    // The lent bytes are exactly what the ToString path used to render.
    assert_eq!(
        visited,
        vec![("db_operation".to_owned(), "insert".to_owned())]
    );
}

#[test]
fn span_durations_projects_from_component_arc() {
    use metered::{DynamicExponentialHistogram, Family, entry::metric};
    use metered_tracing::{SpanDurations, metered_info_span};
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
        // A measurable lifetime keeps the duration sum assertion deterministic.
        metered_info_span!(DbLabels; db_operation = "insert".to_owned())
            .in_scope(|| std::thread::sleep(std::time::Duration::from_millis(1)));
    });

    let mut registry = Registry::new();
    registry.register(metric("db_duration_seconds").source(&db.duration));
    let text = registry.encode_to_string().expect("render duration");
    // Name existence alone would pass on an empty family; pin the one close
    // that was projected, with its real (non-zero) wall-clock duration.
    assert_eq!(
        sample_value(&text, "db_duration_seconds_count{db_operation=\"insert\"}"),
        Some("1"),
        "projected family should have recorded exactly the span close:\n{text}"
    );
    assert!(
        sum_value(&text, "db_duration_seconds_sum{db_operation=\"insert\"}") > 0.0,
        "span close should observe the span's wall-clock duration:\n{text}"
    );
}

/// `SpanDurations` is generic over the histogram backend: an SLO alerting
/// contract needs fixed `le` bounds, so the projected family can be a
/// `BucketHistogram` with custom buckets instead of the default dynamic
/// exponential engine.
#[test]
fn span_durations_over_fixed_slo_buckets_records_into_le_bounds() {
    use metered::{Family, entry::metric};
    use metered_tracing::{SpanDurations, metered_info_span};

    fn slo_histogram() -> BucketHistogram {
        BucketHistogram::new(Buckets::custom([0.5, 5.0]))
    }

    struct Db {
        duration: Family<DbLabels, BucketHistogram>,
    }
    let db = Arc::new(Db {
        duration: Family::new_with_constructor_fn(slo_histogram),
    });

    let telemetry = TracingMetrics::builder()
        .recorder(SpanDurations::on(DbLabels::SPAN, &db, |db: &Db| {
            &db.duration
        }))
        .build();

    tracing::subscriber::with_default(tracing_subscriber::registry().with(telemetry), || {
        metered_info_span!(DbLabels; db_operation = "insert".to_owned()).in_scope(|| {});
    });

    let mut registry = Registry::new();
    registry.register(metric("db_duration_seconds").source(&db.duration));
    let text = registry.encode_to_string().expect("render duration");
    // The sub-second span close lands in the first fixed SLO bucket and rolls
    // up cumulatively -- the shape a `le`-based alert rule queries.
    for le in ["0.5", "5", "+Inf"] {
        assert_eq!(
            sample_value(
                &text,
                &format!("db_duration_seconds_bucket{{db_operation=\"insert\",le=\"{le}\"}}")
            ),
            Some("1"),
            "bucket le={le} should carry the observation:\n{text}"
        );
    }
}

#[test]
fn adopted_exemplars_fire_the_retention_hook() {
    use metered::{DynamicExponentialHistogram, Family};
    use metered_tracing::{SpanDurations, metered_info_span};
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
    let duration = BucketHistogram::new(Buckets::seconds_default());

    tracing::subscriber::with_default(subscriber, || {
        let parent =
            tracing::info_span!("parent", trace_id = "parent-trace", span_id = "parent-span");
        let parent_entered = parent.enter();
        observe_with_source(&duration, &recorded, 0.001);

        {
            let child =
                tracing::info_span!("child", trace_id = "child-trace", span_id = "child-span");
            let _child_entered = child.enter();
            observe_with_source(&duration, &recorded, 0.001);
        }

        observe_with_source(&duration, &recorded, 0.001);
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
