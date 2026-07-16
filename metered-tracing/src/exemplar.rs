//! Exemplar capture: turning the trace context (or any local id) carried on a
//! span into an [`Exemplar`](metered::Exemplar) the layer attaches to a duration
//! bucket, and the ambient exemplar context (`TracingExemplarLayer`) that feeds
//! it into `metered`'s thread-local exemplar source while a span is entered.

#[cfg(feature = "exemplar")]
use crate::{SpanFields, TracingMetrics, sanitize_ident};
#[cfg(feature = "exemplar")]
use metered::Exemplar;
#[cfg(feature = "exemplar")]
use metered::bucket_histogram::set_current_exemplar;
#[cfg(feature = "exemplar")]
use std::cell::RefCell;
#[cfg(feature = "exemplar")]
use std::fmt;
#[cfg(feature = "exemplar")]
use std::sync::Arc as StdArc;
#[cfg(feature = "exemplar")]
use tracing::{Id, Subscriber};
#[cfg(feature = "exemplar")]
use tracing_subscriber::layer::{Context, Layer};
#[cfg(feature = "exemplar")]
use tracing_subscriber::registry::LookupSpan;

/// Marker provider for a [`TracingMetrics`](crate::TracingMetrics) layer without
/// exemplar capture.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoExemplarProvider;

/// Configures exemplar capture for a [`TracingMetrics`](crate::TracingMetrics) layer.
pub trait ExemplarProvider: Clone + Send + Sync + 'static {
    /// When `false`, the layer skips field maps and enter/exit exemplar hooks
    /// for spans that no [`SpanMetric`](crate::SpanMetric) matches.
    fn captures_span_fields(&self) -> bool;

    /// The span-field names this provider reads to build an exemplar, when
    /// they are known up front (`FieldExemplarProvider`, behind the
    /// `exemplar` feature, declares its configured list). The layer unions
    /// them into each span's capture
    /// filter and drops every unrequested field before allocation. `None`,
    /// the default, means the set is open-ended: full field capture is kept
    /// for a provider that does not declare its names.
    fn fields_read(&self) -> Option<Vec<String>> {
        None
    }

    /// Returns an exemplar for the currently-entered span (requires `exemplar` feature).
    #[cfg(feature = "exemplar")]
    fn exemplar(&self, fields: &SpanFields) -> Option<Exemplar> {
        let _ = fields;
        None
    }
}

impl ExemplarProvider for NoExemplarProvider {
    fn captures_span_fields(&self) -> bool {
        false
    }
}

/// Builds an exemplar by lifting a set of span fields into exemplar labels
/// (label name = field name).
///
/// Exemplars are not distributed-tracing-specific: an exemplar is any set of
/// labels that points at a concrete observation. Use `["trace_id", "span_id"]`
/// to link to a trace, or a purely local identifier such as `["order_id"]` to
/// jump from a latency bucket to the exact entity behind it -- no trace system
/// required. Whichever of the configured fields are present at close are
/// lifted; if none are, no exemplar is attached.
#[cfg(feature = "exemplar")]
#[derive(Clone, Debug)]
pub struct FieldExemplarProvider {
    fields: Vec<String>,
}

#[cfg(feature = "exemplar")]
impl FieldExemplarProvider {
    /// Creates a provider that lifts the named span fields into exemplar labels.
    pub fn new(fields: impl IntoIterator<Item = impl Into<String>>) -> Self {
        FieldExemplarProvider {
            fields: fields.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(feature = "exemplar")]
impl ExemplarProvider for FieldExemplarProvider {
    fn captures_span_fields(&self) -> bool {
        true
    }

    fn fields_read(&self) -> Option<Vec<String>> {
        Some(self.fields.clone())
    }

    fn exemplar(&self, fields: &SpanFields) -> Option<Exemplar> {
        let labels: Vec<(String, String)> = self
            .fields
            .iter()
            .filter_map(|name| {
                // Exemplar labels are OpenMetrics labels, so a dotted span
                // attribute (semconv `order.id`) is sanitized to a valid label
                // name (`order_id`).
                fields.text(name).map(|value| (sanitize_ident(name), value))
            })
            .collect();
        if labels.is_empty() {
            return None;
        }
        Some(Exemplar {
            labels,
            value: 0.0,
            timestamp_seconds: None,
        })
    }
}

/// Exemplar-only layer (alias for
/// [`TracingMetrics::exemplar_only`](crate::TracingMetrics::exemplar_only)).
#[cfg(feature = "exemplar")]
#[derive(Clone)]
pub struct TracingExemplarLayer<P> {
    inner: TracingMetrics<P>,
}

#[cfg(feature = "exemplar")]
impl<P: ExemplarProvider> fmt::Debug for TracingExemplarLayer<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TracingExemplarLayer").finish()
    }
}

#[cfg(feature = "exemplar")]
impl<P: ExemplarProvider> TracingExemplarLayer<P> {
    /// Creates an exemplar layer from a provider.
    pub fn new(provider: P) -> Self {
        TracingExemplarLayer {
            inner: TracingMetrics::exemplar_only(provider),
        }
    }
}

#[cfg(feature = "exemplar")]
impl<S, P> Layer<S> for TracingExemplarLayer<P>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    P: ExemplarProvider,
{
    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        self.inner.on_new_span(attrs, id, ctx);
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        self.inner.on_record(id, values, ctx);
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        self.inner.on_close(id, ctx);
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        self.inner.on_enter(id, ctx);
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        self.inner.on_exit(id, ctx);
    }
}

#[cfg(feature = "exemplar")]
thread_local! {
    static EXEMPLAR_STACK: RefCell<Vec<Option<StdArc<Exemplar>>>> =
        const { RefCell::new(Vec::new()) };
}

/// Pushes the entered span's exemplar onto the ambient stack and republishes the
/// current top to `metered`'s thread-local exemplar source.
#[cfg(feature = "exemplar")]
pub(crate) fn push_exemplar(exemplar: Option<Exemplar>) {
    EXEMPLAR_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        stack.push(exemplar.map(StdArc::new));
        sync_current_exemplar(&stack);
    });
}

/// Pops the exited span's exemplar and republishes the new top (the parent's).
#[cfg(feature = "exemplar")]
pub(crate) fn pop_exemplar() {
    EXEMPLAR_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        stack.pop();
        sync_current_exemplar(&stack);
    });
}

#[cfg(feature = "exemplar")]
fn sync_current_exemplar(stack: &[Option<StdArc<Exemplar>>]) {
    set_current_exemplar(
        stack
            .last()
            .and_then(|slot| slot.as_ref().map(|arc| arc.as_ref().clone())),
    );
}
