//! The `tracing-subscriber` [`Layer`] implementation on
//! [`TracingMetrics`]: captures span fields and drives the recorders on span
//! close.

use crate::SpanFields;
use crate::TracingMetrics;
use crate::exemplar::ExemplarProvider;
#[cfg(feature = "exemplar")]
use crate::exemplar::{pop_exemplar, push_exemplar};
use crate::fields::{FieldDemand, SpanFieldVisitor};
use metered::Exemplar;
use std::sync::Arc;
use std::time::Instant;
use tracing::subscriber::Interest;
use tracing::{Id, Metadata, Subscriber};
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

    /// A per-layer [`Filter`](tracing_subscriber::layer::Filter) scoping this
    /// layer to the callsites it can actually use: spans a recorder claims by
    /// name (or every span, when the exemplar provider captures span fields).
    ///
    /// **Mount the layer with this filter**
    /// (`metrics.with_filter(metrics.recorded_spans_filter())`), not bare.
    /// A bare mount reports the default `Interest::always` for every callsite
    /// in the process, which defeats tracing's per-callsite cache: every
    /// *disabled* event and span (h2/hyper/tower emit hundreds per RPC at
    /// trace level) then pays full dynamic filter dispatch on the hot path.
    ///
    /// A per-layer filter is deliberately used instead of implementing
    /// [`Layer::enabled`]/[`Layer::register_callsite`] on the layer itself:
    /// those have *global* semantics — a plain layer returning `false` /
    /// `Interest::never` can veto an event or span for **every** layer in the
    /// subscriber (`Vec<Layer>::enabled` is `all(..)`), silently dropping log
    /// lines and exported spans other layers wanted. The per-layer filter
    /// scopes the disinterest to this layer alone while still restoring the
    /// callsite cache.
    pub fn recorded_spans_filter(&self) -> RecordedSpansFilter {
        RecordedSpansFilter {
            dispatch: Arc::clone(&self.dispatch),
            all_spans: self.exemplar.captures_span_fields(),
        }
    }
}

/// The per-layer filter returned by
/// [`TracingMetrics::recorded_spans_filter`]: interested only in spans whose
/// name is in the recorder dispatch table (or all spans when the exemplar
/// provider captures span fields), and never in events.
#[derive(Clone, Debug)]
pub struct RecordedSpansFilter {
    dispatch: Arc<crate::dispatch::DispatchTable>,
    all_spans: bool,
}

impl RecordedSpansFilter {
    fn wants(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_span() && (self.all_spans || self.dispatch.contains_key(metadata.name()))
    }
}

impl<S: Subscriber> tracing_subscriber::layer::Filter<S> for RecordedSpansFilter {
    fn enabled(&self, metadata: &Metadata<'_>, _cx: &Context<'_, S>) -> bool {
        self.wants(metadata)
    }

    fn callsite_enabled(&self, metadata: &'static Metadata<'static>) -> Interest {
        if self.wants(metadata) {
            Interest::always()
        } else {
            Interest::never()
        }
    }
}

impl<S, P> Layer<S> for TracingMetrics<P>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    P: ExemplarProvider,
{
    fn on_new_span(&self, attrs: &tracing::span::Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        // One dispatch lookup per span: the matched recorder indices and the
        // capture demand are cached on the span so record/close do not
        // re-resolve the name.
        let route = self.dispatch.get(attrs.metadata().name());
        // Capture fields when a span metric needs them for labels, or when an
        // exemplar provider needs them for trace context -- filtered either
        // way to the fields those consumers actually read.
        let (recorders, demand) = match route {
            Some(route) => (
                Some(Arc::clone(&route.recorders)),
                Some(Arc::clone(&route.demand)),
            ),
            None if self.exemplar.captures_span_fields() => {
                (None, Some(Arc::clone(&self.exemplar_demand)))
            }
            None => (None, None),
        };
        let mut state = SpanState::new(recorders, demand);
        if let Some(captured) = state.fields.as_mut() {
            attrs.record(&mut SpanFieldVisitor::new(
                &mut captured.fields,
                &captured.demand,
            ));
        }
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(state);
        }
    }

    fn on_record(&self, id: &Id, values: &tracing::span::Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            let mut extensions = span.extensions_mut();
            if let Some(state) = extensions.get_mut::<SpanState>() {
                if let Some(captured) = state.fields.as_mut() {
                    values.record(&mut SpanFieldVisitor::new(
                        &mut captured.fields,
                        &captured.demand,
                    ));
                }
            }
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            let extensions = span.extensions();
            if let Some(state) = extensions.get::<SpanState>() {
                if let (Some(recorders), Some(fields)) = (
                    state.recorders.as_ref(),
                    state.fields.as_ref().map(|captured| &captured.fields),
                ) {
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
                .and_then(|captured| self.exemplar.exemplar(&captured.fields))
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
    fields: Option<CapturedFields>,
}

/// A span's field capture: the values recorded so far plus the demand set
/// (shared with the routing table) that filters what gets captured.
#[derive(Debug)]
struct CapturedFields {
    demand: Arc<FieldDemand>,
    fields: SpanFields,
}

impl SpanState {
    fn new(recorders: Option<Arc<[usize]>>, demand: Option<Arc<FieldDemand>>) -> Self {
        SpanState {
            started_at: Instant::now(),
            recorders,
            fields: demand.map(|demand| CapturedFields {
                demand,
                fields: SpanFields::default(),
            }),
        }
    }
}
