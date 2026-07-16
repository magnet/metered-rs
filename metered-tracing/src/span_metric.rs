//! [`SpanMetric`]: a semantic family of span-derived metrics (request count +
//! duration histogram), labeled from span fields with per-label cardinality
//! bounds.

use crate::SpanFields;
use crate::recorder::{SpanFieldsError, SpanRecorder, observe_sampled};
use metered::family::MetricConstructor;
use metered::interner::OTHER;
use metered::{
    BoundedValues, BucketHistogram, Buckets, Counter, Exemplar, Family, Help, LabelSet,
    MetricSchema, MetricTree, MetricValues, join_name,
};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

/// The dynamic label set a [`SpanMetric`] keys its families on.
///
/// Preserves interned identity end-to-end: `values` are the `Arc<str>`s handed
/// out by [`BoundedValues`] (or a label's default, or an unbounded rendering),
/// so the close path never re-materializes a value it already interned.
/// `names` is one `Arc` shared by every key of a [`SpanMetric`], cloned (not
/// reallocated) per close. At scrape time
/// [`for_each_label`](LabelSet::for_each_label) lends the interned names and
/// values as `&str`; this type never allocates a string of its own.
#[derive(Clone, Debug)]
pub(crate) struct SpanLabelSet {
    /// Label names in declaration order, shared across all keys of one metric.
    names: Arc<[String]>,
    /// Label values in declaration order, one per name.
    values: Box<[Arc<str>]>,
}

/// Identity is the values alone: within one [`Family`] every key carries the
/// same `names` (the owning [`SpanMetric`]'s shared `Arc`), so hashing or
/// comparing them would only re-walk identical strings on the close hot path.
impl PartialEq for SpanLabelSet {
    fn eq(&self, other: &Self) -> bool {
        self.values == other.values
    }
}

impl Eq for SpanLabelSet {}

impl Hash for SpanLabelSet {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.values.hash(state);
    }
}

impl LabelSet for SpanLabelSet {
    fn for_each_label(&self, f: &mut dyn FnMut(&str, &str)) {
        for (name, value) in self.names.iter().zip(&self.values) {
            f(name, value);
        }
    }
}

/// Default per-label cap on distinct span-sourced values before overflowing to
/// `_OTHER`. Span field values can be attacker-influenced (RPC method names, URL
/// paths, ...), so a [`SpanMetric`] bounds them by default to keep one hostile
/// caller from exploding series cardinality.
pub const DEFAULT_LABEL_VALUE_CAP: usize = 256;

/// Default bound on distinct series (label-value combinations) a [`SpanMetric`]
/// tracks before folding novel combinations into the all-`_OTHER` series; see
/// [`SpanMetricBuilder::max_series`].
///
/// Per-label caps bound each label's *value set*, not the *series count*: `N`
/// bounded labels capped at `K` values still admit up to `(K + 1)^N` distinct
/// combinations under hostile field values. 1024 is chosen as the default
/// because it is an order of magnitude above the legitimate combination count
/// of typical semantic span metrics (tens of methods times a handful of
/// statuses), while keeping the worst-case exposition tractable: at ~18
/// samples per series (a counter plus a default-bucket histogram), 1024 series
/// render roughly 18k lines per scrape for one metric.
///
/// The bound is approximate under concurrent first-sightings: closes can
/// overshoot by at most one series per racing thread; folded series and
/// overflow accounting are unaffected.
pub const DEFAULT_MAX_SERIES: usize = 1024;

/// Maps a span field onto a metric label, with a default when it is absent and
/// an optional distinct-value cap. Builder-side, so it stays `Clone` (the live
/// interner lives in [`BoundedLabel`]).
#[derive(Clone, Debug)]
struct LabelSpec {
    label: String,
    field: String,
    default: String,
    /// Distinct field-value cap (`None` disables bounding for this label).
    cap: Option<usize>,
}

/// A resolved span-field -> label mapping with its own cardinality bound.
///
/// Built from a [`LabelSpec`] at [`SpanMetricBuilder::build`] time: field-sourced
/// values are interned through [`BoundedValues`], collapsing to `_OTHER` past the
/// cap, so an unbounded/untrusted span field cannot explode the family's series.
struct BoundedLabel {
    field: String,
    default: Arc<str>,
    bound: Option<BoundedValues>,
}

impl BoundedLabel {
    fn from_spec(spec: LabelSpec) -> Self {
        BoundedLabel {
            field: spec.field,
            default: Arc::from(spec.default),
            bound: spec.cap.map(BoundedValues::new),
        }
    }

    /// Resolves this label's value from `fields`, bounding a field-sourced value
    /// to the configured cap. The `default` (used when the field is absent) is a
    /// fixed, trusted string and is never counted against the cap.
    ///
    /// Bounded and default values are `Arc` clones -- the interned identity
    /// flows straight into the family key with no string allocation. Only an
    /// explicitly unbounded label allocates here, since its values are by
    /// definition never interned.
    fn value(&self, fields: &SpanFields) -> Arc<str> {
        let Some(value) = fields.value(&self.field) else {
            return Arc::clone(&self.default);
        };
        match (&self.bound, value.as_text()) {
            // Text-captured values intern straight from the borrowed str,
            // skipping the intermediate `to_text` allocation.
            (Some(interner), Some(text)) => interner.bound(text),
            (Some(interner), None) => interner.bound(&value.to_text()),
            (None, Some(text)) => Arc::from(text),
            (None, None) => Arc::from(value.to_text()),
        }
    }
}

/// A semantic family of span-derived metrics, e.g. RPC server or DB client.
///
/// Build one with [`SpanMetric::for_span`], hand a clone to a
/// [`TracingMetrics`](crate::TracingMetrics) layer, and place another clone in a
/// `metered` view under the semantic name you want (`rpc_server`, `db_client`,
/// ...). On every close of a matching span it records a request count and a
/// duration histogram, labeled from the configured span fields.
///
/// The measured duration is the span's **total lifetime** -- open to close,
/// including any time an async task spent idle (not polled) -- not just the
/// time spent inside `enter`/`exit` scopes.
///
/// # Cardinality bounds: per-label values and total series
///
/// Two independent bounds keep hostile span fields from exploding cardinality:
///
/// - **Per-label value caps** shape each label's *value set*: a bounded label
///   interns at most its cap of distinct field-sourced values, collapsing the
///   rest to `_OTHER` (see [`DEFAULT_LABEL_VALUE_CAP`]).
/// - **[`max_series`](SpanMetricBuilder::max_series)** bounds the *count* of
///   distinct label-value combinations. Per-label caps alone still admit a
///   Cartesian product of combinations (`N` labels at cap `K` allow up to
///   `(K + 1)^N` series). Once the metric tracks `max_series` distinct
///   combinations (default [`DEFAULT_MAX_SERIES`]), a close with a **novel**
///   combination is folded into the all-`_OTHER` series -- every label at
///   `_OTHER`, including explicitly unbounded ones -- and counted on the
///   `<name>_overflowed_spans_total` counter. Already-tracked combinations
///   keep recording normally. The bound is approximate under concurrent
///   first-sightings: closes can overshoot by at most one series per racing
///   thread; folded series and overflow accounting are unaffected.
#[derive(Clone)]
pub struct SpanMetric {
    inner: Arc<SpanMetricInner>,
}

struct SpanMetricInner {
    span_name: String,
    help: Option<Help>,
    /// Label names in declaration order, allocated once at build time and
    /// shared (`Arc` clone) into every [`SpanLabelSet`] key.
    label_names: Arc<[String]>,
    labels: Vec<BoundedLabel>,
    /// One family keyed by [`SpanLabelSet`], each member owning the request
    /// counter *and* the duration histogram for its series -- so a close is
    /// one family lock, one hash, and the key is stored once.
    members: Family<SpanLabelSet, SpanMember, MemberConstructor>,
    /// Bound on distinct series; see [`SpanMetricBuilder::max_series`].
    max_series: usize,
    /// The all-`_OTHER` key novel label sets fold into once `max_series`
    /// distinct series exist, built once so the fold path never re-interns.
    overflow_key: SpanLabelSet,
    /// Closes folded into `overflow_key` because the series bound was hit.
    /// Exported as the `<name>_overflowed_spans` counter family.
    overflowed: AtomicU64,
}

/// One series' metrics under a single [`SpanLabelSet`] key: the request
/// counter and the duration histogram that previously lived in two separate
/// families keyed by the same labels.
///
/// Data-only: naming (`<name>_requests` / `<name>_duration_seconds`) is joined
/// once per scrape by [`SpanMetric`]. The [`MetricTree`] impl exists so
/// [`Family`] can forward housekeep; describe/collect are not on the scrape
/// path (and deliberately do nothing).
struct SpanMember {
    requests: AtomicU64,
    duration: BucketHistogram,
}

impl MetricTree for SpanMember {
    fn describe(&self, _: &str, _: &[(&str, &str)], _: &mut MetricSchema) {}

    fn collect(&self, _: &str, _: &[(&str, &str)], _: &mut MetricValues) {}

    fn housekeep(&self) {
        if self.requests.needs_housekeep() {
            self.requests.housekeep();
        }
        if self.duration.needs_housekeep() {
            self.duration.housekeep();
        }
    }

    fn needs_housekeep(&self) -> bool {
        self.requests.needs_housekeep() || self.duration.needs_housekeep()
    }
}

impl SpanMetric {
    /// Starts a [`SpanMetric`] that matches spans named `span_name`.
    pub fn for_span(span_name: impl Into<String>) -> SpanMetricBuilder {
        SpanMetricBuilder {
            span_name: span_name.into(),
            help: None,
            buckets: Buckets::seconds_default(),
            labels: Vec::new(),
            max_series: DEFAULT_MAX_SERIES,
        }
    }

    /// The tracing span name this metric is derived from.
    pub fn span_name(&self) -> &str {
        &self.inner.span_name
    }

    fn label_values(&self, fields: &SpanFields) -> SpanLabelSet {
        // One `label_values` result keys the one member holding both the
        // `requests` counter and the `duration` histogram, so the two rendered
        // families can never drift (`_OTHER` vs raw) for one close.
        SpanLabelSet {
            names: Arc::clone(&self.inner.label_names),
            values: self
                .inner
                .labels
                .iter()
                .map(|label| label.value(fields))
                .collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn series_count(&self) -> usize {
        self.inner.members.len()
    }
}

impl fmt::Debug for SpanMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpanMetric")
            .field("span_name", &self.inner.span_name)
            .field("series", &self.inner.members.len())
            .finish()
    }
}

impl MetricTree for SpanMetric {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        // Join once per scrape: members are data-only and cannot carry the
        // pre-joined names through `Family`'s generic `MetricTree` walk.
        let requests = join_name(name, "requests");
        let duration = join_name(name, "duration_seconds");
        let overflowed = join_name(name, "overflowed_spans");
        if let Some(help) = &self.inner.help {
            schema.set_help_for(&requests, help.clone());
            schema.set_help_for(&duration, help.clone());
        }
        schema.set_unit_for(&duration, "seconds");
        schema.set_help_for(
            &overflowed,
            "Span closes folded into the all-_OTHER series because the metric \
             was at its max_series bound",
        );
        self.inner
            .members
            .with_describe_member(labels, |all, member| {
                member.requests.describe(&requests, all, schema);
                member.duration.describe(&duration, all, schema);
            });
        self.inner.overflowed.describe(&overflowed, labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        // Join once per scrape (not once per series): walking members directly
        // keeps the wire identical to the dual-family layout while avoiding
        // 2 × series_count `format!` allocs inside each member.
        let requests = join_name(name, "requests");
        let duration = join_name(name, "duration_seconds");
        let overflowed = join_name(name, "overflowed_spans");
        self.inner.members.for_each_series(labels, |all, member| {
            member.requests.collect(&requests, all, values);
            member.duration.collect(&duration, all, values);
        });
        self.inner.overflowed.collect(&overflowed, labels, values);
    }

    fn housekeep(&self) {
        self.inner.members.housekeep();
    }

    fn needs_housekeep(&self) -> bool {
        self.inner.members.needs_housekeep()
    }
}

impl SpanRecorder for SpanMetric {
    fn span_name(&self) -> &str {
        &self.inner.span_name
    }

    /// The configured label source fields: the only span fields this
    /// recorder ever reads at close.
    fn fields_read(&self) -> Option<Vec<String>> {
        Some(
            self.inner
                .labels
                .iter()
                .map(|label| label.field.clone())
                .collect(),
        )
    }

    fn record_close(
        &self,
        fields: &SpanFields,
        duration_seconds: f64,
        exemplar: Option<Exemplar>,
    ) -> Result<Option<Exemplar>, SpanFieldsError> {
        // Stringly labels render any captured value, so this recorder has no
        // conversion contract to violate.
        let labels = self.label_values(fields);
        // `take` instead of `clone`: at most one of the paths below runs the
        // closure, so the exemplar moves into whichever member records.
        let mut exemplar = exemplar;
        let mut record = |member: &SpanMember| {
            Counter::incr(&member.requests);
            observe_sampled(&member.duration, duration_seconds, exemplar.take())
        };

        // Hot path: an existing series records both metrics under one family
        // read lock and one key hash.
        if let Some(adopted) = self.inner.members.with_existing(&labels, &mut record) {
            return Ok(adopted);
        }
        // Novel combination: mint it while under the series bound. The check
        // does not serialize closes, so concurrent first-sightings can admit
        // a few series past the bound (at most one per racing thread).
        if self.inner.members.len() < self.inner.max_series {
            return Ok(self.inner.members.with(&labels, record));
        }
        // At the bound: fold the close into the all-`_OTHER` series and count
        // it, instead of minting yet another combination.
        Counter::incr(&self.inner.overflowed);
        Ok(self.inner.members.with(&self.inner.overflow_key, record))
    }
}

/// Builder for a [`SpanMetric`].
#[derive(Clone, Debug)]
pub struct SpanMetricBuilder {
    span_name: String,
    help: Option<Help>,
    buckets: Buckets,
    labels: Vec<LabelSpec>,
    max_series: usize,
}

impl SpanMetricBuilder {
    /// Sets the OpenMetrics `# HELP` text for the emitted families.
    pub fn help(mut self, help: impl Into<Help>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Sets the duration histogram buckets (defaults to second-based buckets).
    pub fn duration_buckets(mut self, buckets: Buckets) -> Self {
        self.buckets = buckets;
        self
    }

    /// Bounds the number of distinct series (label-value combinations) this
    /// metric tracks, defaulting to [`DEFAULT_MAX_SERIES`].
    ///
    /// Per-label caps ([`label_capped`](SpanMetricBuilder::label_capped) and
    /// friends) bound each label's value set; this bounds their product. Once
    /// `max_series` distinct combinations exist, a close with a novel
    /// combination folds into the all-`_OTHER` series (every label at
    /// `_OTHER`, including unbounded ones) and increments the
    /// `<name>_overflowed_spans_total` counter, while already-tracked
    /// combinations keep recording normally. The fold series does not count
    /// against the bound. A bound of `0` folds every close.
    ///
    /// The bound is approximate under concurrent first-sightings: closes can
    /// overshoot by at most one series per racing thread; folded series and
    /// overflow accounting are unaffected.
    pub fn max_series(mut self, max_series: usize) -> Self {
        self.max_series = max_series;
        self
    }

    /// Maps span field `from_field` onto metric `label`, using `"unset"` when
    /// the field is absent at close time. Field-sourced values are bounded to
    /// [`DEFAULT_LABEL_VALUE_CAP`] distinct values (overflow collapses to
    /// `_OTHER`).
    ///
    /// # Panics
    ///
    /// Panics when `label` is already declared on this builder (as does every
    /// other `label_*` method): duplicate label names would render invalid
    /// OpenMetrics series, so the declaration bug fails at wiring time.
    pub fn label(self, label: impl Into<String>, from_field: impl Into<String>) -> Self {
        self.label_or(label, from_field, "unset")
    }

    /// Like [`label`](SpanMetricBuilder::label) but with an explicit `default`
    /// for spans that never set the field.
    ///
    /// The label's field-sourced values are bounded to
    /// [`DEFAULT_LABEL_VALUE_CAP`] distinct values (overflow collapses to
    /// `_OTHER`); for a different bound declare the label with
    /// [`label_capped`](SpanMetricBuilder::label_capped) or
    /// [`label_unbounded`](SpanMetricBuilder::label_unbounded) instead.
    pub fn label_or(
        self,
        label: impl Into<String>,
        from_field: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        self.push_label(label, from_field, default, Some(DEFAULT_LABEL_VALUE_CAP))
    }

    /// Like [`label_or`](SpanMetricBuilder::label_or) with an explicit
    /// distinct-value cap instead of [`DEFAULT_LABEL_VALUE_CAP`]. Field values
    /// beyond the cap collapse to `_OTHER`, so an attacker-influenced span
    /// field cannot explode series cardinality; raise the cap for a
    /// legitimately high-cardinality-but-trusted dimension.
    pub fn label_capped(
        self,
        label: impl Into<String>,
        from_field: impl Into<String>,
        default: impl Into<String>,
        cap: usize,
    ) -> Self {
        self.push_label(label, from_field, default, Some(cap))
    }

    /// Like [`label_or`](SpanMetricBuilder::label_or) without any cardinality
    /// bound. Use only when the value set is trusted and naturally small
    /// (e.g. a fixed status enum); an unbounded untrusted field is a
    /// cardinality-explosion risk.
    pub fn label_unbounded(
        self,
        label: impl Into<String>,
        from_field: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        self.push_label(label, from_field, default, None)
    }

    /// # Panics
    ///
    /// Panics when `label` is already declared on this builder: a duplicate
    /// label name would render two identical label keys on every series --
    /// invalid OpenMetrics exposition -- so the declaration bug fails fast at
    /// wiring time instead of reaching a scrape.
    fn push_label(
        mut self,
        label: impl Into<String>,
        from_field: impl Into<String>,
        default: impl Into<String>,
        cap: Option<usize>,
    ) -> Self {
        let label = label.into();
        assert!(
            !self.labels.iter().any(|spec| spec.label == label),
            "label `{label}` is declared twice on the span metric for `{}`",
            self.span_name,
        );
        self.labels.push(LabelSpec {
            label,
            field: from_field.into(),
            default: default.into(),
            cap,
        });
        self
    }

    /// Finalizes the [`SpanMetric`].
    pub fn build(self) -> SpanMetric {
        let label_names: Arc<[String]> =
            self.labels.iter().map(|spec| spec.label.clone()).collect();
        let members = Family::new_with_constructor_and_label_names(
            MemberConstructor {
                buckets: self.buckets,
            },
            label_names.iter().cloned(),
        );
        // The fold target for series overflow: every label -- bounded or not
        // -- at the `_OTHER` overflow value, one shared `Arc<str>` per slot.
        let other: Arc<str> = Arc::from(OTHER);
        let overflow_key = SpanLabelSet {
            names: Arc::clone(&label_names),
            values: self.labels.iter().map(|_| Arc::clone(&other)).collect(),
        };
        let labels = self
            .labels
            .into_iter()
            .map(BoundedLabel::from_spec)
            .collect();
        SpanMetric {
            inner: Arc::new(SpanMetricInner {
                span_name: self.span_name,
                help: self.help,
                label_names,
                labels,
                members,
                max_series: self.max_series,
                overflow_key,
                overflowed: AtomicU64::new(0),
            }),
        }
    }
}

/// Sanitizes an identifier (e.g. a span-field name into an exemplar label
/// name): any character that is not an ASCII letter, digit, or `_` becomes `_`.
#[cfg(feature = "exemplar")]
pub(crate) fn sanitize_ident(span_name: &str) -> String {
    span_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Builds a fresh [`SpanMember`] when a new series is first observed (and the
/// family's describe prototype), carrying the configured duration buckets.
#[derive(Clone, Debug)]
struct MemberConstructor {
    buckets: Buckets,
}

impl MetricConstructor<SpanMember> for MemberConstructor {
    fn new_metric(&self) -> SpanMember {
        SpanMember {
            requests: AtomicU64::new(0),
            duration: BucketHistogram::new(self.buckets.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::FieldValue;

    fn fields_with(field: &str, value: &str) -> SpanFields {
        SpanFields::from_pairs([(field, FieldValue::Str(value.to_owned()))])
    }

    #[test]
    fn span_label_values_are_bounded_to_cap_plus_other_by_default() {
        use metered::Registry;
        use metered_om::OpenMetricsRegistryExt;

        // A span field is attacker-influenced; the default cap must keep 300
        // distinct values from creating 300 series.
        let metric = SpanMetric::for_span("rpc.server")
            .duration_buckets(Buckets::custom([5.0]))
            .label("rpc_method", "rpc.method")
            .build();

        for i in 0..300 {
            metric
                .record_close(
                    &fields_with("rpc.method", &format!("method-{i}")),
                    1.0,
                    None,
                )
                .expect("stringly labels never violate a conversion contract");
        }

        // DEFAULT_LABEL_VALUE_CAP distinct interned values + one `_OTHER` bucket.
        assert_eq!(
            metric.series_count(),
            DEFAULT_LABEL_VALUE_CAP + 1,
            "span-sourced label values are bounded to cap + _OTHER"
        );

        // Pin exact `_OTHER` sample lines: presence alone would still pass if
        // recorded values were zeroed. 300 closes with cap 256 → 44 folded.
        let mut registry = Registry::new();
        registry.register(metered::entry::metric("rpc_server").source(&metric));
        let text = registry.encode_to_string().expect("render span metric");
        assert!(
            text.contains("rpc_server_requests_total{rpc_method=\"_OTHER\"} 44\n"),
            "overflow series must carry the folded request count:\n{text}"
        );
        assert!(
            text.contains("rpc_server_duration_seconds_sum{rpc_method=\"_OTHER\"} 44\n"),
            "overflow series must carry the folded duration sum:\n{text}"
        );
        assert!(
            text.contains("rpc_server_duration_seconds_count{rpc_method=\"_OTHER\"} 44\n"),
            "overflow series must carry the folded duration count:\n{text}"
        );
    }

    #[test]
    #[should_panic(expected = "label `rpc_method` is declared twice")]
    fn duplicate_label_declarations_are_rejected_at_build_time() {
        // Two declarations of the same label name would emit
        // `{rpc_method="...",rpc_method="..."}` on every series -- invalid
        // exposition -- so the builder rejects the second declaration.
        let _ = SpanMetric::for_span("rpc.server")
            .label("rpc_method", "rpc.method")
            .label_or("rpc_method", "rpc.method.name", "unset");
    }

    #[test]
    fn label_capped_bounds_to_the_declared_cap() {
        let metric = SpanMetric::for_span("rpc.server")
            .label_capped("rpc_method", "rpc.method", "unset", 4)
            .build();

        for i in 0..10 {
            metric
                .record_close(
                    &fields_with("rpc.method", &format!("method-{i}")),
                    0.001,
                    None,
                )
                .expect("stringly labels never violate a conversion contract");
        }

        assert_eq!(
            metric.series_count(),
            4 + 1,
            "an explicit cap bounds to cap + _OTHER"
        );
    }

    #[test]
    fn past_max_series_novel_combinations_fold_into_the_all_other_series() {
        use metered::Registry;
        use metered_om::OpenMetricsRegistryExt;

        let metric = SpanMetric::for_span("rpc.server")
            .duration_buckets(Buckets::custom([5.0]))
            .label("rpc_method", "rpc.method")
            .max_series(2)
            .build();
        let close = |method: &str, seconds: f64| {
            metric
                .record_close(&fields_with("rpc.method", method), seconds, None)
                .expect("stringly labels never violate a conversion contract");
        };

        // Two distinct combinations fill the bound...
        close("method-a", 1.0);
        close("method-b", 1.0);
        // ...an existing series keeps recording normally at the bound...
        close("method-a", 2.0);
        // ...and novel combinations fold into the all-`_OTHER` series (which
        // does not count against the bound) instead of minting new ones.
        close("method-c", 8.0);
        close("method-d", 8.0);

        assert_eq!(
            metric.series_count(),
            3,
            "the two admitted series plus the fold series, nothing else"
        );

        // Pin the full rendered document: the fold series carries the folded
        // observations, the admitted series their own, and the overflow
        // counter the fold count.
        let mut registry = Registry::new();
        registry.register(metered::entry::metric("rpc_server").source(&metric));
        let text = registry.encode_to_string().expect("render span metric");
        assert_eq!(
            text,
            "\
# TYPE rpc_server_duration_seconds histogram
# UNIT rpc_server_duration_seconds seconds
rpc_server_duration_seconds_bucket{rpc_method=\"_OTHER\",le=\"5\"} 0
rpc_server_duration_seconds_bucket{rpc_method=\"_OTHER\",le=\"+Inf\"} 2
rpc_server_duration_seconds_sum{rpc_method=\"_OTHER\"} 16
rpc_server_duration_seconds_count{rpc_method=\"_OTHER\"} 2
rpc_server_duration_seconds_bucket{rpc_method=\"method-a\",le=\"5\"} 2
rpc_server_duration_seconds_bucket{rpc_method=\"method-a\",le=\"+Inf\"} 2
rpc_server_duration_seconds_sum{rpc_method=\"method-a\"} 3
rpc_server_duration_seconds_count{rpc_method=\"method-a\"} 2
rpc_server_duration_seconds_bucket{rpc_method=\"method-b\",le=\"5\"} 1
rpc_server_duration_seconds_bucket{rpc_method=\"method-b\",le=\"+Inf\"} 1
rpc_server_duration_seconds_sum{rpc_method=\"method-b\"} 1
rpc_server_duration_seconds_count{rpc_method=\"method-b\"} 1
# HELP rpc_server_overflowed_spans Span closes folded into the all-_OTHER series because the metric was at its max_series bound
# TYPE rpc_server_overflowed_spans counter
rpc_server_overflowed_spans_total 2
# TYPE rpc_server_requests counter
rpc_server_requests_total{rpc_method=\"_OTHER\"} 2
rpc_server_requests_total{rpc_method=\"method-a\"} 2
rpc_server_requests_total{rpc_method=\"method-b\"} 1
# EOF
"
        );
    }

    #[test]
    fn the_fold_series_sets_every_label_to_other_including_unbounded_ones() {
        // The fold key must be closed over *all* labels: leaving an unbounded
        // label at its live value would let each fold mint a fresh series,
        // defeating the bound it implements.
        let metric = SpanMetric::for_span("rpc.server")
            .label("rpc_method", "rpc.method")
            .label_unbounded("rpc_peer", "rpc.peer", "unset")
            .max_series(0)
            .build();

        metric
            .record_close(
                &SpanFields::from_pairs([
                    ("rpc.method", FieldValue::Str("CreateOrder".to_owned())),
                    ("rpc.peer", FieldValue::Str("10.0.0.1:9999".to_owned())),
                ]),
                0.001,
                None,
            )
            .expect("stringly labels never violate a conversion contract");

        assert_eq!(metric.series_count(), 1, "everything folds at a 0 bound");
        let mut values = MetricValues::new();
        metric.collect("rpc_server", &[], &mut values);
        let request_labels: Vec<_> = values
            .samples()
            .iter()
            .filter(|sample| sample.name == "rpc_server_requests_total")
            .map(|sample| sample.labels.clone())
            .collect();
        assert_eq!(
            request_labels,
            vec![vec![
                ("rpc_method".to_owned(), "_OTHER".to_owned()),
                ("rpc_peer".to_owned(), "_OTHER".to_owned()),
            ]],
            "bounded and unbounded labels alike fold to _OTHER"
        );
    }

    #[test]
    fn unbounded_label_opts_out_of_the_cap() {
        let metric = SpanMetric::for_span("rpc.server")
            .label_unbounded("rpc_method", "rpc.method", "unset")
            .build();

        for i in 0..(DEFAULT_LABEL_VALUE_CAP + 50) {
            metric
                .record_close(
                    &fields_with("rpc.method", &format!("method-{i}")),
                    0.001,
                    None,
                )
                .expect("stringly labels never violate a conversion contract");
        }

        assert_eq!(
            metric.series_count(),
            DEFAULT_LABEL_VALUE_CAP + 50,
            "an explicitly unbounded label keeps every distinct value"
        );
    }
}
