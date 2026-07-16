use metered::{entry::metric, Registry};
use metered_om::OpenMetricsRegistryExt;
use metered_tracing::{SpanMetric, TracingMetrics};
use tracing_subscriber::prelude::*;

fn record_span_and_render() -> Result<String, std::fmt::Error> {
    // One semantic family per span kind: matches `rpc.server` spans and labels
    // the metric from the span's semconv fields. Hand a clone to the layer, keep
    // one to place in the view.
    let rpc = SpanMetric::for_span("rpc.server")
        .help("RPC server calls")
        .label("rpc_method", "rpc.method")
        .label_or("rpc_status", "rpc.grpc.status_code", "OK")
        .build();

    let layer = TracingMetrics::builder().recorder(rpc.clone()).build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let span = tracing::info_span!(
            "rpc.server",
            rpc.method = "CreateOrder",
            rpc.grpc.status_code = "OK"
        );
        let _entered = span.enter();
    });

    let mut registry = Registry::new();
    registry.register(metric("rpc_server").source(&rpc));

    // Emits `rpc_server_requests_total{rpc_method,rpc_status}` and a
    // `rpc_server_duration_seconds` histogram -- a distinct, semantically named
    // family rather than a generic span blob.
    registry.encode_to_string()
}
