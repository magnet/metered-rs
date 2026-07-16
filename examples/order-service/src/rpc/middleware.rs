//! The RPC metrics middleware: a cross-cutting layer, separate from the server.
//!
//! The metrics are *normal* `metered` families keyed by [`RpcLabels`] (one series
//! per method/status):
//!
//! - `requests` is a counter the middleware **bumps directly** in [`call`], since
//!   it knows the method and the outcome;
//! - `duration` is a `Family<_, DynamicExponentialHistogram>` (metered's native
//!   auto-scaling exponential histogram) that `metered-tracing` times via a
//!   stateless [`SpanDurations`] adapter: the adapter, built at wiring time from
//!   a shared `Arc` of this layer and a projection to this family, observes each
//!   `rpc.server` span's open-to-close duration (with a lock-free sampled trace
//!   exemplar) keyed by the same labels.
//!
//! Both metrics are owned here and mounted in the layer's own view. The
//! span/metric contract is single-sourced by `#[derive(SpanLabels)]` on
//! [`RpcLabels`]: the `#[span("otel.field")]` mapping generates the call-site span
//! opener, the `record_*` setter, and `FromSpanFields` (so the adapter rebuilds
//! the typed key from the closed span), so span field and metric label can't drift.

use super::RpcStatus;
use metered::{
    Counter, DynamicExponentialHistogram, Family, LabelSet, MetricTreeView, MetricsView,
};
use metered_tracing::{SpanDurations, SpanLabels, SpanRecorder, metered_info_span};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::Span;

const RPC_SERVICE: &str = "shop.v1.OrderService";

/// The labels for the `rpc.server` span and its metrics: one series per method
/// and status. A normal `#[derive(LabelSet)]` key (constructed every call), with
/// `#[derive(SpanLabels)]` mapping each field to its OTel semconv span field.
#[derive(Clone, PartialEq, Eq, Hash, LabelSet, SpanLabels)]
#[span(
    name = "rpc.server",
    help = "RPC server calls handled by the transport layer"
)]
struct RpcLabels {
    #[span("rpc.method")]
    rpc_method: String,
    // `on_close`: the status isn't known when the span opens, so it starts
    // `Empty` and is set in `classify` (via the generated `record_rpc_status`)
    // before the span closes; `default = "OK"` covers a span that never set it.
    #[span("rpc.grpc.status_code", default = "OK", on_close)]
    rpc_status: RpcStatus,
}

/// A method-agnostic RPC metrics layer, constructed once and shared. This *is*
/// the middleware: it owns the `rpc.server` metrics and the whole span lifecycle.
/// The transport just calls `call` with a handler.
pub struct RpcMetricsLayer {
    /// Bumped directly in `call` -- the middleware knows the method + outcome.
    requests: Family<RpcLabels, AtomicU64>,
    /// The duration histogram, a plain field owned here and mounted in this
    /// layer's view; the tracing adapter holds a shared `Arc` of the *layer* and
    /// projects to this family to time span closes into it.
    duration: Family<RpcLabels, DynamicExponentialHistogram>,
}

impl Default for RpcMetricsLayer {
    fn default() -> Self {
        RpcMetricsLayer::new()
    }
}

impl RpcMetricsLayer {
    pub fn new() -> Self {
        RpcMetricsLayer {
            requests: Family::default(),
            duration: Family::default(),
        }
    }

    /// A stateless adapter, built at wiring time from a shared `Arc` of this
    /// layer, that projects to the duration family and times `rpc.server` span
    /// closes into it. Handed to the layer with `.recorder`; it is not stored in
    /// the middleware. Takes `&Arc<Self>` because the recorder captures the layer
    /// handle, never an `Arc` around the metric.
    pub(crate) fn duration_adapter(self: &Arc<Self>) -> impl SpanRecorder + 'static + use<> {
        SpanDurations::on(RpcLabels::SPAN, self, |layer: &RpcMetricsLayer| {
            &layer.duration
        })
    }

    /// Wraps `handler` with the `rpc.server` span: open, run, classify, then bump
    /// the request counter directly. The duration is recorded by the adapter on
    /// close; this method never touches it.
    pub(crate) fn call(
        &self,
        method: &'static str,
        handler: impl FnOnce() -> RpcStatus,
    ) -> RpcStatus {
        // A real RPC boundary starts or continues a trace here; the span's trace
        // ids become the exemplar attached to `rpc_server_duration_seconds`.
        let (trace_id, span_id) = Self::mint_trace_context();
        let span = Self::open_span(method, &trace_id, &span_id);
        let status = span.in_scope(handler);
        Self::classify(&span, status);
        self.requests.with(
            &RpcLabels {
                rpc_method: method.to_owned(),
                rpc_status: status,
            },
            Counter::incr,
        );
        status
    }

    /// Opens the `rpc.server` span through the generated opener. `rpc_method` is
    /// the typed eager label; the rest is span-only context for an OTLP exporter.
    /// `rpc.grpc.status_code` stays `Empty` until [`classify`](Self::classify).
    fn open_span(method: &'static str, trace_id: &str, span_id: &str) -> Span {
        metered_info_span!(
            RpcLabels;
            rpc_method = method.to_owned(),
            "rpc.system" = "grpc",
            "rpc.service" = RPC_SERVICE,
            "trace_id" = trace_id,
            "span_id" = span_id,
            "otel.status_code" = tracing::field::Empty,
            "error.type" = tracing::field::Empty
        )
    }

    /// Classifies the outcome at close: the `rpc_status` label through the
    /// generated typed setter (so the adapter reads it), plus trace-only context.
    fn classify(span: &Span, status: RpcStatus) {
        RpcLabels::record_rpc_status(span, status);
        span.record("otel.status_code", status.otel_status_code());
        span.record("error.type", status.error_type());
    }

    fn mint_trace_context() -> (String, String) {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        (format!("{n:032x}"), format!("{n:016x}"))
    }
}

/// Both metrics sit under the `server` segment, so they export with the RPC
/// semantic-convention names `rpc_server_requests` and
/// `rpc_server_duration_seconds` (the service is a `service` label, not a name
/// prefix). Both are plain owned families read straight from the layer.
impl MetricsView for RpcMetricsLayer {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        let mut view = MetricTreeView::with_prefix("server");
        view.register(
            metered::entry::metric("requests")
                .select(|layer: &RpcMetricsLayer| &layer.requests)
                .help(RpcLabels::HELP),
        );
        view.register(
            metered::entry::metric("duration_seconds")
                .select(|layer: &RpcMetricsLayer| &layer.duration)
                .help(RpcLabels::HELP)
                .unit("seconds"),
        );
        view
    }
}
