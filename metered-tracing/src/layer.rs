//! The `tracing-subscriber` [`Layer`] implementation on
//! [`TracingMetrics`]: captures span fields and drives the recorders on span
//! close.

use crate::exemplar::ExemplarProvider;
#[cfg(feature = "exemplar")]
use crate::exemplar::{pop_exemplar, push_exemplar};
use crate::fields::SpanFieldVisitor;
use crate::SpanFields;
use crate::TracingMetrics;
use metered::Exemplar;
use std::sync::Arc;
use std::time::Instant;
use tracing::{Id, Subscriber};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

impl<P: ExemplarProvider> TracingMetrics<P> {
    /// Builds the exemplar to attach to a matched span's duration at close, from
    /// the trace fields captured on the span.
    #[cfg(feature = "exemplar")]
    fn close_exemplar(&self, fields: &SpanFields) -> Option<Exemplar> {
        self.exemplar.exemplar(fields)
    }

    #[cfg(not(feature = "exemplar"))]
    fn close_exemplar(&self, _fields: &SpanFields) -> Option<Exemplar> {
        None
    }
}

impl<S, P> Layer<S> for TracingMetrics<P>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    P: ExemplarProvider,
{
    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        // One dispatch lookup per span: the matched recorder indices are cached
        // on the span so the close does not re-resolve the name.
        let recorders = self.dispatch.get(attrs.metadata().name()).cloned();
        // Capture fields when a span metric needs them for labels, or when an
        // exemplar provider needs them for trace context.
        let capture = recorders.is_some() || self.exemplar.captures_span_fields();
        let mut state = SpanState::new(recorders, capture);
        if let Some(fields) = state.fields.as_mut() {
            attrs.record(&mut SpanFieldVisitor::new(fields));
        }
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(state);
        }
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut extensions = span.extensions_mut();
            if let Some(state) = extensions.get_mut::<SpanState>() {
                if let Some(fields) = state.fields.as_mut() {
                    values.record(&mut SpanFieldVisitor::new(fields));
                }
            }
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            let extensions = span.extensions();
            if let Some(state) = extensions.get::<SpanState>() {
                if let (Some(recorders), Some(fields)) =
                    (state.recorders.as_ref(), state.fields.as_ref())
                {
                    let duration_seconds = state.started_at.elapsed().as_secs_f64();
                    // The exemplar depends only on the captured fields, so it is
                    // built once and cloned into each recorder of the fan-out.
                    let exemplar = self.close_exemplar(fields);
                    // Every recorder registered for this span name observes the
                    // close; duplicate names are a deliberate fan-out.
                    for &index in recorders.iter() {
                        match self.recorders[index].record_close(
                            fields,
                            duration_seconds,
                            exemplar.clone(),
                        ) {
                            Ok(Some(adopted)) => {
                                if let Some(hook) = &self.on_exemplar_adopted {
                                    hook(&adopted);
                                }
                            }
                            Ok(None) => {}
                            // A captured field violated the recorder's typed
                            // label contract: skip the observation and count it
                            // under the bounded malformed-span metric instead
                            // of letting a defaulted label pose as real data.
                            Err(_) => self.malformed.record(span.name()),
                        }
                    }
                }
            }
        }
    }

    #[cfg(feature = "exemplar")]
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if !self.exemplar.captures_span_fields() {
            return;
        }
        let exemplar = ctx.span(id).and_then(|span| {
            span.extensions()
                .get::<SpanState>()
                .and_then(|state| state.fields.as_ref())
                .and_then(|fields| self.exemplar.exemplar(fields))
        });
        push_exemplar(exemplar);
    }

    #[cfg(feature = "exemplar")]
    fn on_exit(&self, _id: &Id, _ctx: Context<'_, S>) {
        if self.exemplar.captures_span_fields() {
            pop_exemplar();
        }
    }
}

#[derive(Debug)]
struct SpanState {
    started_at: Instant,
    /// The recorder indices matched at open (`None` when no recorder claims
    /// the span name), shared with the dispatch table so the close needs no
    /// second lookup.
    recorders: Option<Arc<[usize]>>,
    fields: Option<SpanFields>,
}

impl SpanState {
    fn new(recorders: Option<Arc<[usize]>>, capture_fields: bool) -> Self {
        SpanState {
            started_at: Instant::now(),
            recorders,
            fields: capture_fields.then(SpanFields::default),
        }
    }
}
