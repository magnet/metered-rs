//! [`SpanMetric`]: a semantic family of span-derived metrics (request count +
//! duration histogram), labeled from span fields with per-label cardinality
//! bounds.

use crate::recorder::{observe_sampled, SpanFieldsError, SpanRecorder};
use crate::SpanFields;
use metered::family::MetricConstructor;
use metered::{
    join_name, BoundedValues, BucketHistogram, Buckets, Counter, Exemplar, Family, Help,
    MetricSchema, MetricTree, MetricValues,
};
use std::fmt;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

/// The dynamic label set a [`SpanMetric`] emits: label name to value.
pub(crate) type SpanLabelValues = Vec<(String, String)>;

/// Default per-label cap on distinct span-sourced values before overflowing to
/// `_OTHER`. Span field values can be attacker-influenced (RPC method names, URL
/// paths, ...), so a [`SpanMetric`] bounds them by default to keep one hostile
/// caller from exploding series cardinality.
pub const DEFAULT_LABEL_VALUE_CAP: usize = 256;

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
    label: String,
    field: String,
    default: String,
    bound: Option<BoundedValues>,
}

impl BoundedLabel {
    fn from_spec(spec: LabelSpec) -> Self {
        BoundedLabel {
            label: spec.label,
            field: spec.field,
            default: spec.default,
            bound: spec.cap.map(BoundedValues::new),
        }
    }

    /// Resolves this label's value from `fields`, bounding a field-sourced value
    /// to the configured cap. The `default` (used when the field is absent) is a
    /// fixed, trusted string and is never counted against the cap.
    fn value(&self, fields: &SpanFields) -> String {
        let Some(value) = fields.value(&self.field) else {
            return self.default.clone();
        };
        match &self.bound {
            Some(interner) => match value.as_text() {
                // Text-captured values intern straight from the borrowed str,
                // skipping the intermediate `to_text` allocation.
                Some(text) => interner.bound(text).to_string(),
                None => interner.bound(&value.to_text()).to_string(),
            },
            None => value.to_text(),
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
#[derive(Clone)]
pub struct SpanMetric {
    inner: Arc<SpanMetricInner>,
}

struct SpanMetricInner {
    span_name: String,
    help: Option<Help>,
    labels: Vec<BoundedLabel>,
    requests: Family<SpanLabelValues, AtomicU64>,
    duration: Family<SpanLabelValues, BucketHistogram, HistogramConstructor>,
}

impl SpanMetric {
    /// Starts a [`SpanMetric`] that matches spans named `span_name`.
    pub fn for_span(span_name: impl Into<String>) -> SpanMetricBuilder {
        SpanMetricBuilder {
            span_name: span_name.into(),
            help: None,
            buckets: Buckets::seconds_default(),
            labels: Vec::new(),
        }
    }

    /// The tracing span name this metric is derived from.
    pub fn span_name(&self) -> &str {
        &self.inner.span_name
    }

    fn label_values(&self, fields: &SpanFields) -> SpanLabelValues {
        // One `label_values` result drives both the `requests` counter and the
        // `duration` histogram, so a bounded value keeps them on the same series
        // (no `_OTHER`/raw drift between the two families).
        self.inner
            .labels
            .iter()
            .map(|label| (label.label.clone(), label.value(fields)))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn series_count(&self) -> usize {
        self.inner.requests.len()
    }
}

impl fmt::Debug for SpanMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpanMetric")
            .field("span_name", &self.inner.span_name)
            .field("series", &self.inner.requests.len())
            .finish()
    }
}

impl MetricTree for SpanMetric {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        let requests = join_name(name, "requests");
        let duration = join_name(name, "duration_seconds");
        if let Some(help) = &self.inner.help {
            schema.set_help_for(&requests, help.clone());
            schema.set_help_for(&duration, help.clone());
        }
        schema.set_unit_for(&duration, "seconds");
        self.inner.requests.describe(&requests, labels, schema);
        self.inner.duration.describe(&duration, labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.inner
            .requests
            .collect(&join_name(name, "requests"), labels, values);
        self.inner
            .duration
            .collect(&join_name(name, "duration_seconds"), labels, values);
    }
}

impl SpanRecorder for SpanMetric {
    fn span_name(&self) -> &str {
        &self.inner.span_name
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
        self.inner.requests.with(&labels, Counter::incr);
        Ok(self.inner.duration.with(&labels, move |h| {
            observe_sampled(h, duration_seconds, exemplar)
        }))
    }
}

/// Builder for a [`SpanMetric`].
#[derive(Clone, Debug)]
pub struct SpanMetricBuilder {
    span_name: String,
    help: Option<Help>,
    buckets: Buckets,
    labels: Vec<LabelSpec>,
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

    /// Maps span field `from_field` onto metric `label`, using `"unset"` when
    /// the field is absent at close time. Field-sourced values are bounded to
    /// [`DEFAULT_LABEL_VALUE_CAP`] distinct values (overflow collapses to
    /// `_OTHER`).
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

    fn push_label(
        mut self,
        label: impl Into<String>,
        from_field: impl Into<String>,
        default: impl Into<String>,
        cap: Option<usize>,
    ) -> Self {
        self.labels.push(LabelSpec {
            label: label.into(),
            field: from_field.into(),
            default: default.into(),
            cap,
        });
        self
    }

    /// Finalizes the [`SpanMetric`].
    pub fn build(self) -> SpanMetric {
        let label_names: Vec<String> = self.labels.iter().map(|spec| spec.label.clone()).collect();
        let requests = Family::with_label_names(label_names.clone());
        let duration = Family::new_with_constructor_and_label_names(
            HistogramConstructor {
                buckets: self.buckets,
            },
            label_names,
        );
        let labels = self
            .labels
            .into_iter()
            .map(BoundedLabel::from_spec)
            .collect();
        SpanMetric {
            inner: Arc::new(SpanMetricInner {
                span_name: self.span_name,
                help: self.help,
                labels,
                requests,
                duration,
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

#[derive(Clone, Debug)]
struct HistogramConstructor {
    buckets: Buckets,
}

impl MetricConstructor<BucketHistogram> for HistogramConstructor {
    fn new_metric(&self) -> BucketHistogram {
        BucketHistogram::new(self.buckets.clone())
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
        // A span field is attacker-influenced; the default cap must keep 300
        // distinct values from creating 300 series.
        let metric = SpanMetric::for_span("rpc.server")
            .label("rpc_method", "rpc.method")
            .build();

        for i in 0..300 {
            metric
                .record_close(
                    &fields_with("rpc.method", &format!("method-{i}")),
                    0.001,
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

        // The overflow really collapses to `_OTHER`, and both families agree.
        let mut values = MetricValues::new();
        metric.collect("rpc_server", &[], &mut values);
        let has_other = values.samples().iter().any(|sample| {
            sample
                .labels
                .iter()
                .any(|(key, value)| key == "rpc_method" && value == "_OTHER")
        });
        assert!(has_other, "overflow values collapse to _OTHER");
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
