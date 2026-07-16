//! Cross-cutting telemetry policy: the span/metric naming translation and the
//! exemplar provider shared by the whole service.
//!
//! There is deliberately **no** telemetry bundle here. Each component owns the
//! [`SpanMetric`](metered_tracing::SpanMetric)s for the spans *it* emits (next
//! to the span itself, so the span name + fields have one source of truth) and
//! mounts them in its own metric view. The routing layer is assembled from
//! those owned metrics at the composition root ([`crate::app::App::run_with_tracing`]);
//! this module only holds the policy that cuts across every component.
//!
//! # Naming policy: two conventions, one translation
//!
//! Spans use **OpenTelemetry semantic conventions** (dotted: `rpc.method`,
//! `db.operation`, `order.category`) -- what OTLP/Tempo expect. Metrics use
//! **OpenMetrics** (snake_case, `_total`, labels like `rpc_method`) -- what
//! Prometheus expects. The [`SpanMetric`](metered_tracing::SpanMetric) is the
//! translator: it maps a dotted span field to a snake metric label, and the exemplar
//! provider sanitizes dotted fields into valid label names too. A span metric
//! labels only a *curated, low-cardinality* subset; the rest of the span's
//! semconv fields stay span-only as trace/exporter context.

use metered_tracing::FieldExemplarProvider;

/// The exemplar policy for the whole service: span durations gain an exemplar
/// from whichever of these fields the span carries -- trace ids on the RPC
/// boundary (distributed), or a purely local `order.id` on the order operation
/// (no trace system needed). The dotted `order.id` field is sanitized to an
/// `order_id` exemplar label.
pub fn exemplar_provider() -> FieldExemplarProvider {
    FieldExemplarProvider::new(["trace_id", "span_id", "order.id"])
}
