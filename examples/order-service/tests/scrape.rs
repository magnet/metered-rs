use metered_om::{OpenMetricsDocument, OpenMetricsExt};
use order_service_demo::app::{run_demo_workload, App};

#[test]
fn demo_scrape_shows_span_derived_and_directly_owned_metrics() {
    let app = App::demo();
    app.run_with_tracing(|| run_demo_workload(&app, 100));

    let text = app.encode_to_string().unwrap();
    let doc = OpenMetricsDocument::parse(&text).unwrap();

    // Service identity (a custom type implementing `Info`).
    let service = doc.sample("service_info").unwrap();
    assert_eq!(service.value, "1");
    assert_eq!(service.label("service_name"), Some("order-service"));
    assert_eq!(service.label("region"), Some("eu-west-1"));

    // Transport metrics use the RPC semantic-convention family name shared by
    // every service (`rpc_server_requests_total`) -- the service is a `service`
    // label, not a name prefix, and the method/status are labels too.
    let rpc = doc.sample("rpc_server_requests_total").unwrap();
    assert_eq!(rpc.value, "100");
    assert_eq!(rpc.label("service"), Some("order-service"));
    assert_eq!(rpc.label("rpc_method"), Some("CreateOrder"));
    assert_eq!(rpc.label("rpc_status"), Some("OK"));
    assert_eq!(
        doc.sample("rpc_server_duration_seconds_count")
            .unwrap()
            .value,
        "100"
    );
    // Counts alone survive an `observe(0.0)` regression; positive sums pin the
    // fact that real wall-clock durations flowed into every histogram.
    assert!(sum_sample(&doc, "rpc_server_duration_seconds_sum") > 0.0);

    // The dynamic exponential duration histogram carries a trace exemplar on the
    // bucket the observation landed in. The exemplar is *sampled* (one per bucket,
    // kept on power-of-two count crossings) rather than the most recent, so assert
    // its shape and internal consistency, not a pinned id: a 32-hex `trace_id` and
    // 16-hex `span_id` minted from the same counter, and no local `order_id`.
    let rpc_exemplar = exemplar_for_bucket(&doc, "rpc_server_duration_seconds");
    let trace_id = rpc_exemplar
        .label("trace_id")
        .expect("rpc exemplar trace_id");
    let span_id = rpc_exemplar.label("span_id").expect("rpc exemplar span_id");
    assert_eq!(trace_id.len(), 32);
    assert_eq!(span_id.len(), 16);
    assert_eq!(
        u128::from_str_radix(trace_id, 16).unwrap(),
        u128::from(u64::from_str_radix(span_id, 16).unwrap()),
        "trace_id and span_id should be minted from the same request counter"
    );
    assert_eq!(rpc_exemplar.label("order_id"), None);

    // Exemplars are not trace-only: the order operation's latency buckets carry a
    // purely local `order_id`, with no distributed trace context at all.
    let order_exemplars =
        exemplars_for_bucket_family(&doc, "order_service_orders_create_duration_seconds");
    assert!(
        !order_exemplars.is_empty(),
        "expected local order_id exemplars on orders_create duration buckets"
    );
    for exemplar in order_exemplars {
        assert!(exemplar.label("order_id").is_some());
        assert_eq!(exemplar.label("trace_id"), None);
        assert_eq!(exemplar.label("span_id"), None);
    }

    // Business operation metrics, derived from the `orders.create_order` span.
    // These are service-specific: they live in the `order_service_` namespace and
    // carry *no* `service` label -- the prefix is the identity.
    let orders = doc
        .sample("order_service_orders_create_requests_total")
        .unwrap();
    assert_eq!(orders.label("service"), None);
    assert_eq!(
        sum_samples(&doc, "order_service_orders_create_requests_total"),
        100
    );
    assert!(doc
        .samples_named("order_service_orders_create_requests_total")
        .iter()
        .any(|sample| sample.label("category") == Some("books")
            && sample.label("channel") == Some("web")));
    assert_eq!(
        sum_samples(&doc, "order_service_orders_create_duration_seconds_count"),
        100
    );
    assert!(sum_sample(&doc, "order_service_orders_create_duration_seconds_sum") > 0.0);

    // DB query metrics, derived from the `db.query` span. DB is a cross-service
    // semantic-convention family, so it keeps the shared name and a `service` label.
    let db = doc.sample("db_client_requests_total").unwrap();
    assert_eq!(db.value, "100");
    assert_eq!(db.label("service"), Some("order-service"));
    assert_eq!(db.label("db_operation"), Some("insert_order"));
    assert_eq!(
        doc.sample("db_client_duration_seconds_count")
            .unwrap()
            .value,
        "100"
    );
    assert!(sum_sample(&doc, "db_client_duration_seconds_sum") > 0.0);

    // Background job runs, derived from the `jobs.run` span: one job ran, the
    // rest found an empty queue.
    assert_eq!(
        sum_samples(&doc, "order_service_jobs_run_requests_total"),
        100
    );
    assert_eq!(
        sample_with_label(
            &doc,
            "order_service_jobs_run_requests_total",
            "outcome",
            "ran",
        )
        .value,
        "1"
    );
    assert_eq!(
        sample_with_label(
            &doc,
            "order_service_jobs_run_requests_total",
            "outcome",
            "empty",
        )
        .value,
        "99"
    );

    // Directly-owned business counter (flattened: no doubled `orders_` segment).
    assert_eq!(sum_samples(&doc, "order_service_orders_created_total"), 100);

    // Directly-owned gauges (cache size, pool, queue depth).
    assert_eq!(
        doc.sample("order_service_orders_cache_entries")
            .unwrap()
            .value,
        "100"
    );
    assert_eq!(doc.sample("db_pool_in_use").unwrap().value, "0");
    assert_eq!(doc.sample("db_pool_idle").unwrap().value, "4");
    // A gauge synthesized by the view -- no field stores it.
    assert_eq!(
        doc.family("db_pool_utilization").unwrap().metric_type,
        Some(metered::MetricType::Gauge)
    );
    assert_eq!(
        doc.sample("db_pool_utilization")
            .unwrap()
            .value
            .parse::<f64>()
            .unwrap(),
        0.0
    );
    assert_eq!(
        doc.sample("order_service_jobs_queue_depth").unwrap().value,
        "0"
    );

    // Dynamic payment-rail fleet: one labeled series per live rail, metrics read
    // from the rails themselves. One settlement per order; 1 in 10 fails.
    assert_eq!(
        sum_samples(&doc, "order_service_payments_settlements_total"),
        100
    );
    assert_eq!(
        sum_samples(&doc, "order_service_payments_failures_total"),
        10
    );

    // `bnpl` was onboarded at runtime (iteration 50) yet shows up with traffic --
    // the collection picked it up with no extra wiring.
    assert!(
        sample_with_label(
            &doc,
            "order_service_payments_settlements_total",
            "rail",
            "bnpl",
        )
        .value
        .parse::<u64>()
        .unwrap()
            > 0
    );

    // Every rail is at rest after the workload.
    for rail in ["card", "wallet", "bnpl"] {
        assert_eq!(
            sample_with_label(&doc, "order_service_payments_in_flight", "rail", rail).value,
            "0"
        );
    }

    // The old generic `span_closed_total{span_name=...}` blob is gone: spans now
    // feed distinctly-named semantic families.
    assert!(doc.family("spans_span_closed").is_none());
}

fn sum_samples(doc: &OpenMetricsDocument, name: &str) -> u64 {
    doc.samples_named(name)
        .iter()
        .map(|sample| sample.value.parse::<u64>().unwrap())
        .sum()
}

/// The combined value of a family's `_sum` samples across all label sets.
fn sum_sample(doc: &OpenMetricsDocument, name: &str) -> f64 {
    let samples = doc.samples_named(name);
    assert!(!samples.is_empty(), "missing sum sample {name}");
    samples
        .iter()
        .map(|sample| sample.value.parse::<f64>().unwrap())
        .sum()
}

fn sample_with_label<'a>(
    doc: &'a OpenMetricsDocument,
    name: &str,
    label: &str,
    value: &str,
) -> &'a metered_om::OpenMetricsSample {
    doc.samples_named(name)
        .into_iter()
        .find(|sample| sample.label(label) == Some(value))
        .unwrap_or_else(|| panic!("missing sample {} with {}={}", name, label, value))
}

fn exemplar_for_bucket<'a>(
    doc: &'a OpenMetricsDocument,
    family: &str,
) -> &'a metered_om::OpenMetricsExemplar {
    exemplars_for_bucket_family(doc, family)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("missing exemplar for bucket family {family}"))
}

fn exemplars_for_bucket_family<'a>(
    doc: &'a OpenMetricsDocument,
    family: &str,
) -> Vec<&'a metered_om::OpenMetricsExemplar> {
    let bucket = format!("{family}_bucket");
    doc.samples_named(&bucket)
        .into_iter()
        .filter_map(|sample| sample.exemplar.as_ref())
        .collect()
}
