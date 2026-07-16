//! Assembling span recorders into a routing bundle: [`TracingMetrics`], its
//! builder, and the span-name dispatch table the layer consults per span.

use crate::exemplar::{ExemplarProvider, NoExemplarProvider};
use crate::fields::FieldDemand;
use crate::recorder::SpanRecorder;
use metered::{Counter, Exemplar, Family, MetricSchema, MetricTree, MetricValues};
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// A component that emits instrumented spans and *owns* the span metrics for
/// them.
///
/// This is the ecosystem plug-in point and the ownership seam: a crate that
/// emits, say, `rpc.server` spans holds the metrics for those spans as fields
/// and implements `SpanMetricsSource` to contribute recorders. Taking
/// `self: &Arc<Self>` lets an implementation clone the component handle into
/// [`SpanDurations::on`](crate::SpanDurations::on) projections, so the same
/// component instance is exported through its own metric view (where it sits in
/// the tree) *and* fed to the routing layer here (so span closes are recorded)
/// -- the recorders capture the component `Arc`, never `Arc`s around individual
/// metrics. A service assembles the layer by `.source`-ing each shared
/// component; nothing is flattened into a separate telemetry blob.
pub trait SpanMetricsSource {
    /// The recorders this component contributes to the routing layer.
    fn span_recorders(self: &Arc<Self>) -> Vec<Box<dyn SpanRecorder>>;
}

/// A hook fired with each adopted exemplar: the trace-retention seam shared by
/// the builder and [`TracingMetrics`].
pub(crate) type ExemplarAdoptedHook = Arc<dyn Fn(&Exemplar) + Send + Sync>;

/// The bounded malformed-span counter: closes skipped because a captured field
/// violated a recorder's typed label contract, keyed **only** by span name (a
/// fixed set -- the registered recorders), so hostile field values cannot grow
/// its cardinality.
///
/// A cheap clonable handle over shared state: obtain it from
/// [`TracingMetrics::malformed_spans`] and mount it in a metric view (it
/// implements [`MetricTree`]) so skipped closes are visible on the scrape.
#[derive(Clone, Debug)]
pub struct MalformedSpans {
    counts: Arc<Family<(&'static str, String), AtomicU64>>,
}

impl Default for MalformedSpans {
    fn default() -> Self {
        MalformedSpans {
            // Declare the key label up front so the family's schema is
            // complete before the first malformed close.
            counts: Arc::new(Family::with_label_names(["span"])),
        }
    }
}

impl MalformedSpans {
    pub(crate) fn record(&self, span_name: &str) {
        self.counts
            .with(&("span", span_name.to_owned()), Counter::incr);
    }

    /// Total skipped closes across all span names (test/inspection helper).
    pub fn total(&self) -> u64 {
        let mut values = MetricValues::new();
        self.counts.collect("malformed", &[], &mut values);
        values
            .samples()
            .iter()
            .map(|sample| match sample.value {
                metered::MetricSampleValue::UInt(count) => count,
                _ => 0,
            })
            .sum()
    }
}

impl MetricTree for MalformedSpans {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.counts.describe(name, labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.counts.collect(name, labels, values);
    }
}

/// One span name's routing entry: the recorders that claim it, plus the
/// capture filter their field reads (and the exemplar provider's) add up to.
#[derive(Clone, Debug)]
pub(crate) struct SpanRoute {
    /// The recorder indices registered for this span name.
    pub(crate) recorders: Arc<[usize]>,
    /// The union of field names this span's recorders and the exemplar
    /// provider read: the demand set the capture visitor filters against.
    pub(crate) demand: Arc<FieldDemand>,
}

/// The span-name routing table: every recorder registered for a name receives
/// the close. Two recorders may deliberately observe the same span (a
/// [`SpanMetric`](crate::SpanMetric) profile plus a duration adapter, a shadow
/// rollout, ...), so registration order carries no hidden precedence.
///
/// The index lists and demand sets are shared so the layer can cache a span's
/// match on open (refcount bumps) and drive record/close without a second
/// name lookup.
pub(crate) type DispatchTable = HashMap<String, SpanRoute>;

/// A set of span recorders routed by span name; **the** `tracing-subscriber`
/// layer of this crate (it implements
/// [`Layer`](tracing_subscriber::layer::Layer) directly).
///
/// Build it with [`TracingMetrics::builder`], adding each
/// [`SpanMetric`](crate::SpanMetric) you also place in your metric view, then
/// attach a clone to the subscriber: `.with(telemetry.clone())`. With the
/// `exemplar` feature it can carry an [`ExemplarProvider`] so the same layer
/// feeds trace context into histogram exemplars.
#[derive(Clone)]
pub struct TracingMetrics<P = NoExemplarProvider> {
    pub(crate) recorders: Arc<Vec<Box<dyn SpanRecorder>>>,
    /// Span name -> recorder indices: the routing table the layer consults on
    /// every span open. Built once so dispatch is O(1), not a per-span scan.
    pub(crate) dispatch: Arc<DispatchTable>,
    pub(crate) exemplar: P,
    /// Fired when a recorder's exemplar is adopted by its histogram; the seam
    /// trace-retention glue hangs from. Carried through
    /// `with_exemplar_provider` so the hook survives provider swaps.
    pub(crate) on_exemplar_adopted: Option<ExemplarAdoptedHook>,
    /// Closes skipped over a typed-label contract violation, by span name.
    pub(crate) malformed: MalformedSpans,
    /// The exemplar provider's own field demand: the capture filter for spans
    /// no recorder claims (captured only because the provider wants fields).
    pub(crate) exemplar_demand: Arc<FieldDemand>,
}

/// The exemplar provider's field demand: nothing when it does not capture
/// span fields, its declared names when it does, or open-ended capture when
/// a capturing provider leaves its names undeclared.
fn exemplar_demand<P: ExemplarProvider>(provider: &P) -> FieldDemand {
    if !provider.captures_span_fields() {
        return FieldDemand::default();
    }
    match provider.fields_read() {
        Some(names) => FieldDemand::Named(names.into_iter().collect()),
        None => FieldDemand::All,
    }
}

/// Builds the span-name dispatch table. Every recorder registered for a span
/// name is dispatched to -- duplicate names are an explicit fan-out, not a
/// silent drop of whoever registered later. Each route's demand set is the
/// union of its recorders' declared field reads plus the exemplar provider's,
/// so the capture visitor drops everything else before allocation.
fn dispatch_index(
    recorders: &[Box<dyn SpanRecorder>],
    exemplar_demand: &FieldDemand,
) -> DispatchTable {
    let mut index: HashMap<String, Vec<usize>> = HashMap::with_capacity(recorders.len());
    for (position, recorder) in recorders.iter().enumerate() {
        index
            .entry(recorder.span_name().to_owned())
            .or_default()
            .push(position);
    }
    index
        .into_iter()
        .map(|(name, positions)| {
            let mut demand = exemplar_demand.clone();
            for &position in &positions {
                demand.extend(recorders[position].fields_read());
            }
            let route = SpanRoute {
                recorders: positions.into(),
                demand: Arc::new(demand),
            };
            (name, route)
        })
        .collect()
}

impl<P: ExemplarProvider> TracingMetrics<P> {
    fn assemble(
        recorders: Vec<Box<dyn SpanRecorder>>,
        exemplar: P,
        on_exemplar_adopted: Option<ExemplarAdoptedHook>,
    ) -> Self {
        let provider_demand = exemplar_demand(&exemplar);
        let dispatch = dispatch_index(&recorders, &provider_demand);
        TracingMetrics {
            recorders: Arc::new(recorders),
            dispatch: Arc::new(dispatch),
            exemplar,
            on_exemplar_adopted,
            malformed: MalformedSpans::default(),
            exemplar_demand: Arc::new(provider_demand),
        }
    }
}

impl TracingMetrics<NoExemplarProvider> {
    /// Starts an empty builder.
    pub fn builder() -> TracingMetricsBuilder {
        TracingMetricsBuilder::default()
    }

    /// Attaches an exemplar provider so metrics and exemplars share one layer.
    #[cfg(feature = "exemplar")]
    pub fn with_exemplar_provider<P: ExemplarProvider>(self, provider: P) -> TracingMetrics<P> {
        // Each route's capture filter folds in the provider's field demand,
        // so the routing table is rebuilt for the new provider.
        let provider_demand = exemplar_demand(&provider);
        let dispatch = dispatch_index(&self.recorders, &provider_demand);
        TracingMetrics {
            recorders: self.recorders,
            dispatch: Arc::new(dispatch),
            exemplar: provider,
            on_exemplar_adopted: self.on_exemplar_adopted,
            malformed: self.malformed,
            exemplar_demand: Arc::new(provider_demand),
        }
    }
}

#[cfg(feature = "exemplar")]
impl<P: ExemplarProvider> TracingMetrics<P> {
    /// Creates a layer source that only feeds exemplars (no span metrics).
    pub fn exemplar_only(provider: P) -> Self {
        TracingMetrics::assemble(Vec::new(), provider, None)
    }
}

impl Default for TracingMetrics<NoExemplarProvider> {
    fn default() -> Self {
        TracingMetrics::builder().build()
    }
}

impl<P: ExemplarProvider> TracingMetrics<P> {
    /// The bounded malformed-span counter: closes skipped because a captured
    /// field violated a recorder's typed label contract, keyed by span name.
    /// Mount the returned handle in a metric view so the errors are scraped.
    pub fn malformed_spans(&self) -> MalformedSpans {
        self.malformed.clone()
    }

    fn captures_fields(&self) -> bool {
        self.exemplar.captures_span_fields()
    }
}

impl<P: ExemplarProvider> fmt::Debug for TracingMetrics<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TracingMetrics")
            .field("recorders", &self.recorders.len())
            .field("captures_fields", &self.captures_fields())
            .field("on_exemplar_adopted", &self.on_exemplar_adopted.is_some())
            .finish()
    }
}

/// Builder for [`TracingMetrics`].
#[derive(Default)]
pub struct TracingMetricsBuilder {
    recorders: Vec<Box<dyn SpanRecorder>>,
    on_exemplar_adopted: Option<ExemplarAdoptedHook>,
}

impl TracingMetricsBuilder {
    /// Adds one [`SpanRecorder`]: a [`SpanMetric`](crate::SpanMetric) profile,
    /// or a [`SpanDurations`](crate::SpanDurations) adapter that times a span
    /// into a `Family` your component owns. The single "add one recorder"
    /// entry point.
    ///
    /// Several recorders may claim the same span name; every one of them
    /// receives each matching close.
    pub fn recorder(mut self, recorder: impl SpanRecorder + 'static) -> Self {
        self.recorders.push(Box::new(recorder));
        self
    }

    /// Adds the recorders a [`SpanMetricsSource`] component contributes -- the
    /// way a service composes several crates' span telemetry into one layer.
    ///
    /// Takes `&Arc<T>` so the shared component handle can be cloned into the
    /// recorders' projections (the same `Arc` the component is exported through).
    ///
    /// ```
    /// # use metered_tracing::{SpanMetric, SpanMetricsSource, SpanRecorder, TracingMetrics};
    /// # use std::sync::Arc;
    /// struct RpcLayer { server: SpanMetric }
    /// impl SpanMetricsSource for RpcLayer {
    ///     fn span_recorders(self: &Arc<Self>) -> Vec<Box<dyn SpanRecorder>> {
    ///         vec![Box::new(self.server.clone())]
    ///     }
    /// }
    ///
    /// let rpc = Arc::new(RpcLayer { server: SpanMetric::for_span("rpc.server").build() });
    /// let telemetry = TracingMetrics::builder().source(&rpc).build();
    /// ```
    pub fn source<T: SpanMetricsSource>(mut self, component: &Arc<T>) -> Self {
        for recorder in component.span_recorders() {
            self.recorders.push(recorder);
        }
        self
    }

    /// Registers a hook fired whenever a recorder's exemplar is adopted by its
    /// histogram (kept as the visible bucket exemplar). The exemplar's labels
    /// identify the trace (e.g. `trace_id`), so this is the seam for marking
    /// traces for retention against tail-sampling. The hook is preserved when
    /// the built bundle is later given an exemplar provider.
    pub fn on_exemplar_adopted(mut self, hook: impl Fn(&Exemplar) + Send + Sync + 'static) -> Self {
        self.on_exemplar_adopted = Some(Arc::new(hook));
        self
    }

    /// Finalizes the layer source.
    pub fn build(self) -> TracingMetrics<NoExemplarProvider> {
        TracingMetrics::assemble(self.recorders, NoExemplarProvider, self.on_exemplar_adopted)
    }
}
