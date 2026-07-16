use metered::{Registry, entry::metric};
use metered_om::OpenMetricsRegistryExt;
use metered_tracing::{SpanMetric, TracingMetrics};
use tracing_subscriber::prelude::*;

// Semconv-shaped RPC metrics, derived from spans.
//
// A gRPC middleware opens an `rpc.server` span carrying the OpenTelemetry
// semantic fields (service, method, status). A `SpanMetric` turns those spans
// into a semantic family whose labels are the span fields -- the method lives in
// a label, not in the metric name. No hand-written recording is required.

fn grpc_server_metric() -> SpanMetric {
    SpanMetric::for_span("rpc.server")
        .help("gRPC server calls")
        .label("rpc_service", "rpc.service")
        .label("rpc_method", "rpc.method")
        .label_or("grpc_status", "rpc.grpc.status_code", "OK")
        .label_or("error_type", "error.type", "none")
        .build()
}

fn record_ok(layer: &TracingMetrics) {
    let subscriber = tracing_subscriber::registry().with(layer.clone());
    tracing::subscriber::with_default(subscriber, || {
        tracing::info_span!(
            "rpc.server",
            rpc.system = "grpc",
            rpc.service = "orders.v1.OrderService",
            rpc.method = "GetOrder",
            rpc.grpc.status_code = "OK"
        )
        .in_scope(|| {});
    });
}

fn render(rpc: &SpanMetric) -> Result<String, metered::SinkError> {
    let mut registry = Registry::new();
    registry.register(metric("rpc_server").source(rpc));
    registry.encode_to_string()
}
