//! The per-layer [`RecordedSpansFilter`](metered_tracing::RecordedSpansFilter)
//! must scope disinterest to the metrics layer alone: mounting the layer with
//! `with_filter(recorded_spans_filter())` restores the callsite cache for this
//! layer without vetoing spans or events other layers in the same subscriber
//! want to see.

use metered::Registry;
use metered_om::OpenMetricsRegistryExt;
use metered_tracing::{SpanMetric, TracingMetrics};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;

/// Counts every span open and every event delivered to it, standing in for a
/// co-registered fmt/export layer that wants the full stream.
#[derive(Clone, Default)]
struct CountingLayer {
    spans: Arc<AtomicUsize>,
    events: Arc<AtomicUsize>,
}

impl<S: Subscriber> Layer<S> for CountingLayer {
    fn on_new_span(
        &self,
        _attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::Id,
        _ctx: Context<'_, S>,
    ) {
        self.spans.fetch_add(1, Ordering::Relaxed);
    }

    fn on_event(&self, _event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        self.events.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn filtered_mount_does_not_veto_spans_or_events_for_other_layers() {
    let rpc = SpanMetric::for_span("rpc.server")
        .label("rpc_method", "rpc.method")
        .build();
    let metrics = TracingMetrics::builder().recorder(rpc.clone()).build();
    let filter = metrics.recorded_spans_filter();
    let counting = CountingLayer::default();

    let subscriber = tracing_subscriber::registry()
        .with(metrics.with_filter(filter))
        .with(counting.clone());

    tracing::subscriber::with_default(subscriber, || {
        // A span the metrics layer records...
        tracing::info_span!("rpc.server", rpc.method = "CreateOrder").in_scope(|| {});
        // ...a span no recorder claims, and an event the filter is never
        // interested in: the per-layer filter must not veto these globally.
        tracing::info_span!("uninteresting").in_scope(|| {});
        tracing::info!("an event the metrics layer ignores");
    });

    assert_eq!(
        counting.spans.load(Ordering::Relaxed),
        2,
        "co-registered layer sees the unmatched span too"
    );
    assert_eq!(
        counting.events.load(Ordering::Relaxed),
        1,
        "co-registered layer sees events the metrics layer ignores"
    );

    // The filter still lets the matched span through to the metrics layer,
    // and only that one was recorded.
    let mut registry = Registry::new();
    registry.register(metered::entry::metric("rpc_server").source(&rpc));
    let text = registry.encode_to_string().expect("render span metric");
    assert!(
        text.contains("rpc_server_requests_total{rpc_method=\"CreateOrder\"} 1"),
        "matched span recorded through the filtered mount:\n{text}"
    );
}
