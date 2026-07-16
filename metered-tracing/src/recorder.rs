//! The span->metric recording seam.
//!
//! [`SpanRecorder`] is what the [`TracingMetrics`](crate::TracingMetrics)
//! layer drives when a matched span closes. [`SpanDurations`] is a stateless,
//! duration-only adapter over a `Family` a component owns; [`FromSpanFields`]
//! rebuilds its typed label key from the closed span. [`observe_sampled`] is the
//! one place a span-derived duration and its sampled exemplar meet.

use crate::fields::{FieldValueError, SpanFields};
use metered::{BucketHistogram, DynamicExponentialHistogram, Exemplar, Family, LabelSet};
use std::fmt;
use std::hash::Hash;
use std::sync::Arc;

/// A span close was skipped because a captured field did not convert into the
/// recorder's typed label set.
///
/// Surfaced (rather than silently defaulted) so a contract violation between
/// the span producer and the label declaration shows up as a counted error --
/// see [`TracingMetrics::malformed_spans`](crate::TracingMetrics::malformed_spans)
/// -- instead of masquerading as a legitimate series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanFieldsError {
    /// The span field (OTel semconv name) that failed to convert.
    pub field: &'static str,
    /// Why the captured value did not convert.
    pub error: FieldValueError,
}

impl fmt::Display for SpanFieldsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "span field `{}`: {}", self.field, self.error)
    }
}

impl std::error::Error for SpanFieldsError {}

/// A stateless adapter the [`TracingMetrics`](crate::TracingMetrics) layer
/// drives when a matched span closes. It writes into metrics it does not own --
/// it carries only the handles needed to record -- so the metrics live with
/// their component and the adapter is created at wiring time and handed to the
/// layer with [`recorder`](crate::TracingMetricsBuilder::recorder).
///
/// Implemented by [`SpanMetric`](crate::SpanMetric) (count + duration in one
/// owned handle) and by [`SpanDurations`] (a duration-only adapter over a
/// `Family` you own elsewhere).
pub trait SpanRecorder: Send + Sync {
    /// The tracing span name this recorder matches.
    fn span_name(&self) -> &str;

    /// The span-field names [`record_close`](SpanRecorder::record_close)
    /// reads, when they are known up front.
    ///
    /// The layer unions the declared names across every consumer of a span
    /// and drops any other field *before* capture -- no allocation, no
    /// `Debug` formatting -- so declare them when you can.
    /// [`SpanMetric`](crate::SpanMetric) and [`SpanDurations`] (over a
    /// derived label set) do. `None`, the default, means the set is
    /// open-ended: the layer keeps capturing every field for this recorder's
    /// spans, which is always correct, just unfiltered.
    fn fields_read(&self) -> Option<Vec<String>> {
        None
    }

    /// Records one matched span close. On success, returns the exemplar if the
    /// underlying histogram adopted it (kept it as the visible bucket
    /// exemplar), which is the signal a trace-retention hook hangs from. An
    /// `Err` means the close was **skipped** because a captured field violated
    /// the typed label contract; the layer counts it under the bounded
    /// malformed-span metric.
    fn record_close(
        &self,
        fields: &SpanFields,
        duration_seconds: f64,
        exemplar: Option<Exemplar>,
    ) -> Result<Option<Exemplar>, SpanFieldsError>;
}

/// A duration histogram that records an `f64`, optionally tying a sampled
/// exemplar to it. Implemented by both histogram shapes used as duration
/// metrics, so every [`SpanRecorder`] attaches exemplars through one code path
/// rather than each re-deriving the match.
///
/// This is the observation seam [`SpanDurations`] is generic over: any `H`
/// implementing it can back the projected `Family<L, H>` -- the dynamic
/// exponential histogram by default, or a [`BucketHistogram`] when fixed
/// (e.g. SLO) `le` bounds are required.
pub trait ObserveSampled {
    /// Records one observation, in the histogram's base unit (seconds for
    /// span durations).
    fn record(&self, value: f64);
    /// Records with an exemplar; returns whether the exemplar was adopted (won
    /// its bucket's sampling window).
    fn record_with_exemplar(&self, value: f64, exemplar: Exemplar) -> bool;
}

impl ObserveSampled for BucketHistogram {
    fn record(&self, value: f64) {
        self.observe(value);
    }

    fn record_with_exemplar(&self, value: f64, exemplar: Exemplar) -> bool {
        self.observe_with_exemplar(value, exemplar);
        // BucketHistogram keeps last-write-wins exemplars; treat every store as
        // adopted so retention follows the visible exemplar.
        true
    }
}

impl ObserveSampled for DynamicExponentialHistogram {
    fn record(&self, value: f64) {
        self.observe(value);
    }

    fn record_with_exemplar(&self, value: f64, exemplar: Exemplar) -> bool {
        // Span-derived exemplars are first-in-window; an "interesting" upgrade
        // is a caller concern the duration adapter does not model.
        self.observe_with_exemplar(value, exemplar, false)
    }
}

/// Records one observation into a duration histogram, tying any exemplar to the
/// value it points at (this observation). The single place span-derived
/// durations and their exemplars meet, shared by every [`SpanRecorder`].
/// Returns the adopted exemplar when the histogram kept it (the retention-hook
/// signal).
pub(crate) fn observe_sampled(
    histogram: &impl ObserveSampled,
    value: f64,
    exemplar: Option<Exemplar>,
) -> Option<Exemplar> {
    match exemplar {
        Some(mut exemplar) => {
            exemplar.value = value;
            let adopted = histogram.record_with_exemplar(value, exemplar.clone());
            adopted.then_some(exemplar)
        }
        None => {
            histogram.record(value);
            None
        }
    }
}

/// Builds a typed label set from a closed span's recorded fields.
///
/// Generated by `#[derive(SpanLabels)]` from the `#[span("otel.field")]`
/// mapping. Each field converts through
/// [`FromFieldValue`](crate::FromFieldValue), so a natively-typed span value
/// (an `i64`, a `u64`, a `bool`) becomes the typed label directly -- no
/// `to_string` -> `FromStr` round-trip -- and a value that does not convert is
/// a [`SpanFieldsError`], not a silently defaulted label: the layer skips the
/// observation and counts it, so a producer/declaration contract violation
/// cannot masquerade as a legitimate series.
pub trait FromSpanFields: Sized {
    /// Reads this label set from a closed span's captured fields.
    fn try_from_span_fields(fields: &SpanFields) -> Result<Self, SpanFieldsError>;

    /// The span-field names
    /// [`try_from_span_fields`](FromSpanFields::try_from_span_fields) reads,
    /// when they are statically known.
    ///
    /// `#[derive(SpanLabels)]` overrides this with the declared
    /// `#[span("...")]` names, letting the layer drop every other field
    /// before capture. `None`, the default, means the set is open-ended: a
    /// hand-written implementation that does not declare its names keeps
    /// full field capture for its spans.
    fn span_field_names() -> Option<&'static [&'static str]> {
        None
    }
}

/// A stateless span-duration **adapter** over a `Family` your component owns.
///
/// Unlike [`SpanMetric`](crate::SpanMetric) (which owns its families and both
/// counts and times), `SpanDurations` owns no metric: it holds an `Arc` of the
/// **containing component** plus a projection to the `Family<L, H>` inside it.
/// Metrics stay plain struct fields (the same fields the component's
/// `MetricsView` exposes for scraping), and no `Arc` ever wraps an individual
/// metric. Build it at wiring time from the component handle the service graph
/// already shares:
///
/// ```ignore
/// .recorder(SpanDurations::on(DbLabels::SPAN, &db, |db: &Db| &db.duration))
/// ```
///
/// On each matching span close it observes the open-to-close duration (with the
/// close exemplar) into the projected family, keyed by `L::from_span_fields`.
///
/// The histogram backend `H` defaults to `metered`'s dynamic exponential
/// histogram, so buckets are auto-scaled log-spaced (the native "exponential"
/// look), and the exemplar is sampled per bucket, lock-free. Any backend
/// implementing [`ObserveSampled`] works: name it explicitly (e.g.
/// `SpanDurations<C, L, BucketHistogram>` over a `Family<L, BucketHistogram>`
/// built with fixed SLO bounds) when the exported `le` buckets must match an
/// alerting contract.
///
/// # Cardinality: typed labels are unbounded
///
/// **This path has no cardinality cap.** Unlike
/// [`SpanMetric`](crate::SpanMetric) -- which bounds every field-sourced label
/// to [`DEFAULT_LABEL_VALUE_CAP`](crate::DEFAULT_LABEL_VALUE_CAP) distinct
/// values and collapses overflow to `_OTHER` -- a `SpanDurations` family keys
/// series by whatever `L` values the spans produce. Every distinct label set
/// is a new series, forever.
///
/// Declare typed labels as **enum-like bounded types** (an enum, a small
/// well-known set parsed via `FromStr`). A `String` label filled from an
/// attacker-influenced span field (URL path, peer-supplied method name,
/// header value, ...) lets one hostile caller mint unbounded series and blow
/// up the registry. Where the value set is closed, make
/// [`FromFieldValue`](crate::FromFieldValue) (via `FromStr`) **reject unknown
/// values** instead of storing them: a failed conversion skips the
/// observation and is counted under the bounded
/// [`malformed_spans`](crate::TracingMetrics::malformed_spans) metric rather
/// than becoming a new series.
pub struct SpanDurations<C, L, H = DynamicExponentialHistogram> {
    span_name: String,
    component: Arc<C>,
    project: fn(&C) -> &Family<L, H>,
}

impl<C, L, H> SpanDurations<C, L, H> {
    /// Builds an adapter timing spans named `span_name` into the family
    /// `project` selects from `component`. Takes `&Arc<C>` and clones
    /// internally; the component is whatever your service graph already shares
    /// (a pool, a sub-service), never the metric itself.
    pub fn on(
        span_name: impl Into<String>,
        component: &Arc<C>,
        project: fn(&C) -> &Family<L, H>,
    ) -> Self {
        SpanDurations {
            span_name: span_name.into(),
            component: Arc::clone(component),
            project,
        }
    }
}

impl<C, L, H> fmt::Debug for SpanDurations<C, L, H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpanDurations")
            .field("span_name", &self.span_name)
            .finish()
    }
}

impl<C, L, H> SpanRecorder for SpanDurations<C, L, H>
where
    C: Send + Sync + 'static,
    L: LabelSet + FromSpanFields + Clone + Hash + Eq + Send + Sync + 'static,
    H: ObserveSampled + 'static,
{
    fn span_name(&self) -> &str {
        &self.span_name
    }

    fn fields_read(&self) -> Option<Vec<String>> {
        L::span_field_names().map(|names| names.iter().map(|name| (*name).to_owned()).collect())
    }

    fn record_close(
        &self,
        fields: &SpanFields,
        duration_seconds: f64,
        exemplar: Option<Exemplar>,
    ) -> Result<Option<Exemplar>, SpanFieldsError> {
        let labels = L::try_from_span_fields(fields)?;
        let family = (self.project)(&*self.component);
        Ok(family.with(&labels, move |h| {
            observe_sampled(h, duration_seconds, exemplar)
        }))
    }
}
