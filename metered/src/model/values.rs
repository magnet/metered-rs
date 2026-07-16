//! The sampled-value model: a scrape-time snapshot of a metric tree's current
//! values, independent of (and downstream of) the [`schema`](crate::schema).
//!
//! A [`MetricSchema`](crate::MetricSchema) describes *shape*; [`MetricValues`]
//! captures the *values* at one instant. The renderer combines the two.

use crate::bucket_histogram::{Exemplar, HistogramSnapshot};
use crate::exponential_histogram::ExponentialSnapshot;
use crate::labels::slices::clone_labels;
use std::fmt::{self, Display};

/// A sampled OpenMetrics document: the current values for a metric tree.
///
/// Scalar metrics are stored as flat [`MetricSample`]s; histograms are kept
/// *structurally* (a [`HistogramValue`]) rather than pre-expanded into bucket
/// samples, so a sink chooses the bucket rendering -- cumulative `le`, or
/// VictoriaMetrics `vmrange` -- at encode time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetricValues {
    samples: Vec<MetricSample>,
    histograms: Vec<HistogramValue>,
}

/// A collected histogram, carried whole so the sink picks the bucket rendering.
#[derive(Clone, Debug, PartialEq)]
pub struct HistogramValue {
    /// Family name (without the `_bucket` / `_sum` / `_count` suffix).
    pub name: String,
    /// Constant/inherited labels for this series.
    pub labels: Vec<(String, String)>,
    /// The recorded distribution.
    pub data: HistogramData,
}

/// The recorded form of a [`HistogramValue`].
#[derive(Clone, Debug, PartialEq)]
pub enum HistogramData {
    /// Classic cumulative `le` buckets (from a bucket histogram).
    Classic(HistogramSnapshot),
    /// Native log-linear buckets (from an exponential histogram); renderable as
    /// `vmrange` directly, or converted to `le`.
    Exponential(ExponentialSnapshot),
}

/// A typed OpenMetrics sample value.
///
/// Keeping the value typed (rather than pre-rendered text) lets alternative
/// renderers format integers and floats their own way, and keeps full counter
/// precision (`u64`) that an `f64` could not represent. The [`Display`]
/// implementation renders the canonical OpenMetrics token.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MetricSampleValue {
    /// A signed integer sample (gauges, deltas).
    Int(i64),
    /// An unsigned integer sample (counters, counts, info/stateset bits).
    UInt(u64),
    /// A floating-point sample (sums, quantiles, fractional gauges).
    Float(f64),
}

impl Display for MetricSampleValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetricSampleValue::Int(value) => write!(f, "{value}"),
            MetricSampleValue::UInt(value) => write!(f, "{value}"),
            // OpenMetrics spells infinities `+Inf` / `-Inf` (Rust's `Display`
            // would emit `inf` / `-inf`). `NaN` already matches.
            MetricSampleValue::Float(value) if value.is_infinite() => {
                f.write_str(if *value > 0.0 { "+Inf" } else { "-Inf" })
            }
            MetricSampleValue::Float(value) => write!(f, "{value}"),
        }
    }
}

macro_rules! sample_value_from {
    ($($ty:ty => $variant:ident as $cast:ty),* $(,)?) => {
        $(
            impl From<$ty> for MetricSampleValue {
                fn from(value: $ty) -> Self {
                    MetricSampleValue::$variant(value as $cast)
                }
            }
        )*
    };
}

sample_value_from! {
    i32 => Int as i64,
    i64 => Int as i64,
    isize => Int as i64,
    u32 => UInt as u64,
    u64 => UInt as u64,
    usize => UInt as u64,
    f64 => Float as f64,
}

impl From<bool> for MetricSampleValue {
    fn from(value: bool) -> Self {
        MetricSampleValue::UInt(value as u64)
    }
}

/// One sampled OpenMetrics series.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSample {
    /// Sample name as it will be rendered (`*_total`, `*_bucket`, ...).
    pub name: String,
    /// Sample labels.
    pub labels: Vec<(String, String)>,
    /// Typed sample value.
    pub value: MetricSampleValue,
    /// Optional OpenMetrics exemplar.
    pub exemplar: Option<MetricExemplar>,
}

/// One sampled exemplar attached to a metric sample.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricExemplar {
    /// Exemplar labels.
    pub labels: Vec<(String, String)>,
    /// Exemplar value.
    pub value: f64,
    /// Optional exemplar timestamp, in seconds.
    pub timestamp: Option<f64>,
}

impl MetricValues {
    /// Creates an empty values collection.
    pub fn new() -> Self {
        MetricValues::default()
    }

    /// Adds one sample with raw sample name.
    pub fn sample(
        &mut self,
        name: &str,
        labels: &[(&str, &str)],
        value: impl Into<MetricSampleValue>,
    ) -> &mut Self {
        self.samples.push(MetricSample {
            name: name.to_owned(),
            labels: clone_labels(labels),
            value: value.into(),
            exemplar: None,
        });
        self
    }

    /// Adds one sample with an exemplar.
    pub fn sample_with_exemplar(
        &mut self,
        name: &str,
        labels: &[(&str, &str)],
        value: impl Into<MetricSampleValue>,
        exemplar: &Exemplar,
    ) -> &mut Self {
        self.samples.push(MetricSample {
            name: name.to_owned(),
            labels: clone_labels(labels),
            value: value.into(),
            exemplar: Some(MetricExemplar {
                labels: exemplar.labels.clone(),
                value: exemplar.value,
                timestamp: exemplar.timestamp_seconds,
            }),
        });
        self
    }

    /// Adds a counter sample (`name_total`).
    pub fn counter(
        &mut self,
        name: &str,
        labels: &[(&str, &str)],
        value: impl Into<MetricSampleValue>,
    ) {
        self.sample(&format!("{name}_total"), labels, value);
    }

    /// Adds a gauge sample.
    pub fn gauge(
        &mut self,
        name: &str,
        labels: &[(&str, &str)],
        value: impl Into<MetricSampleValue>,
    ) {
        self.sample(name, labels, value);
    }

    /// Records a classic (cumulative `le`) histogram, kept whole for the sink
    /// to render.
    pub fn histogram(&mut self, name: &str, labels: &[(&str, &str)], snapshot: &HistogramSnapshot) {
        self.histograms.push(HistogramValue {
            name: name.to_owned(),
            labels: clone_labels(labels),
            data: HistogramData::Classic(snapshot.clone()),
        });
    }

    /// Records a native exponential histogram, kept whole so a sink can render
    /// it as `vmrange` (or convert it to `le`).
    pub fn exponential_histogram(
        &mut self,
        name: &str,
        labels: &[(&str, &str)],
        snapshot: &ExponentialSnapshot,
    ) {
        self.histograms.push(HistogramValue {
            name: name.to_owned(),
            labels: clone_labels(labels),
            data: HistogramData::Exponential(snapshot.clone()),
        });
    }

    /// The collected histograms, kept structurally for the sink to expand.
    pub fn histograms(&self) -> &[HistogramValue] {
        &self.histograms
    }

    /// All samples in insertion order.
    pub fn samples(&self) -> &[MetricSample] {
        &self.samples
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_value_renders_openmetrics_special_floats() {
        assert_eq!(MetricSampleValue::Float(f64::INFINITY).to_string(), "+Inf");
        assert_eq!(
            MetricSampleValue::Float(f64::NEG_INFINITY).to_string(),
            "-Inf"
        );
        assert_eq!(MetricSampleValue::Float(f64::NAN).to_string(), "NaN");
        assert_eq!(MetricSampleValue::Float(0.5).to_string(), "0.5");
        assert_eq!(MetricSampleValue::UInt(7).to_string(), "7");
        assert_eq!(MetricSampleValue::Int(-3).to_string(), "-3");
    }

    #[test]
    fn metric_values_helpers_add_counter_gauge_and_samples() {
        let mut values = MetricValues::new();
        values.counter("requests", &[("svc", "api")], 2u64);
        values.gauge("depth", &[], -3);
        values.sample("plain", &[("kind", "demo")], 7);

        assert_eq!(values.samples().len(), 3);
        assert!(values.samples().iter().any(|sample| {
            sample.name == "requests_total"
                && sample.value == MetricSampleValue::UInt(2)
                && sample.labels == vec![("svc".to_owned(), "api".to_owned())]
        }));
        assert!(values
            .samples()
            .iter()
            .any(|sample| sample.name == "depth" && sample.value == MetricSampleValue::Int(-3)));
        assert!(values
            .samples()
            .iter()
            .any(|sample| sample.name == "plain" && sample.value == MetricSampleValue::Int(7)));
    }

    #[test]
    fn metric_values_can_attach_exemplars_to_samples() {
        let exemplar = Exemplar {
            labels: vec![("trace_id".to_owned(), "abc".to_owned())],
            value: 2.5,
            timestamp_seconds: None,
        };
        let mut values = MetricValues::new();
        values.sample_with_exemplar("latency_bucket", &[("le", "+Inf")], 1, &exemplar);

        let sample = &values.samples()[0];
        assert_eq!(sample.name, "latency_bucket");
        assert_eq!(sample.exemplar.as_ref().unwrap().value, 2.5);
        assert_eq!(sample.exemplar.as_ref().unwrap().timestamp, None);
    }
}
