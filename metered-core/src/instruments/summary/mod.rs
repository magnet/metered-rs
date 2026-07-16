//! A first-class OpenMetrics **summary** instrument and the [`QuantileSource`]
//! seam its quantiles come from.
//!
//! A summary reports pre-computed quantiles (`name{quantile="0.99"}`) plus
//! `name_sum` and `name_count`. Its quantiles are **per-instance and do not
//! aggregate across replicas** -- prefer a histogram unless you specifically
//! need exact per-instance quantiles or legacy dashboard parity.
//!
//! [`Summary`] is a thin read-view over any [`QuantileSource`]; the quantiles
//! are read from a distribution you already record. `metered`'s histograms
//! expose a [`QuantileSource`] via [`BucketQuantiles`]; the exponential
//! histogram is the recommended source (lock-free, bounded memory, bounded
//! relative error). You can also implement [`QuantileSource`] yourself over
//! whatever distribution you record.

mod buckets;

// Exposed (doc-hidden) so downstream crates can derive a summary-shaped
// reading from a histogram's buckets without duplicating the math.
pub use buckets::BucketQuantiles;
#[doc(hidden)]
pub use buckets::quantile_from_buckets;

use crate::labels::slices::with_labels;
use crate::metric_tree::{Metric, MetricType};
use crate::schema::MetricSchema;
use crate::values::MetricValues;

/// The default quantiles a [`Summary`] reports when none are configured.
pub const DEFAULT_QUANTILES: [f64; 4] = [0.5, 0.9, 0.95, 0.99];

/// One consistent reading of a [`QuantileSource`].
///
/// `#[non_exhaustive]`: a future reading attribute can become a new field
/// without a breaking change. Third-party [`QuantileSource`] impls construct
/// one through [`SummaryReading::new`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct SummaryReading {
    /// The value at each requested quantile, in the order requested.
    pub values: Vec<f64>,
    /// Sum of all observed values.
    pub sum: f64,
    /// Number of observations.
    pub count: u64,
}

impl SummaryReading {
    /// Assembles a reading from its parts -- the constructor for third-party
    /// [`QuantileSource`] impls. `values` must hold one entry per requested
    /// quantile, in the order requested.
    pub fn new(values: Vec<f64>, sum: f64, count: u64) -> Self {
        SummaryReading { values, sum, count }
    }
}

/// A read-side source of summary statistics. Read at scrape time; keep it cheap.
pub trait QuantileSource {
    /// Reads the value at each quantile in `quantiles` (returned positionally,
    /// one value per input quantile, in the same order) plus `sum` and `count`,
    /// from a single consistent observation. `NaN` for a quantile with no data.
    fn read_summary(&self, quantiles: &[f64]) -> SummaryReading;
}

/// Drops quantiles outside `0.0..=1.0`.
// Exposed (doc-hidden) for downstream summary-shaped readers.
#[doc(hidden)]
pub fn valid_quantiles(quantiles: impl IntoIterator<Item = f64>) -> Vec<f64> {
    quantiles
        .into_iter()
        .filter(|q| (0.0..=1.0).contains(q))
        .collect()
}

/// An OpenMetrics summary over any [`QuantileSource`].
#[derive(Debug, Clone)]
pub struct Summary<S> {
    source: S,
    quantiles: Vec<f64>,
}

impl<S> Summary<S> {
    /// A summary over `source`, reporting the [`DEFAULT_QUANTILES`].
    pub fn new(source: S) -> Self {
        Summary {
            source,
            quantiles: DEFAULT_QUANTILES.to_vec(),
        }
    }

    /// Overrides the reported quantiles. Values outside `0.0..=1.0` are dropped,
    /// so the source is only ever asked for in-range quantiles.
    pub fn with_quantiles(mut self, quantiles: impl IntoIterator<Item = f64>) -> Self {
        self.quantiles = valid_quantiles(quantiles);
        self
    }

    /// The underlying quantile source.
    pub fn source(&self) -> &S {
        &self.source
    }
}

impl<S: QuantileSource> Metric for Summary<S> {
    fn metric_type(&self) -> MetricType {
        MetricType::Summary
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let reading = self.source.read_summary(&self.quantiles);
        debug_assert_eq!(
            reading.values.len(),
            self.quantiles.len(),
            "QuantileSource::read_summary must return one value per requested quantile"
        );
        for (quantile, value) in self.quantiles.iter().zip(&reading.values) {
            let quantile_str = quantile.to_string();
            let quantile_labels = with_labels(labels, [("quantile", quantile_str.as_str())]);
            values.sample(name, &quantile_labels, *value);
        }
        values.sample(&format!("{name}_sum"), labels, reading.sum);
        values.sample(&format!("{name}_count"), labels, reading.count);
    }

    fn describe_metric(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        // Structural path: check incoming labels for a user `quantile` before
        // compose would shadow it under the type exemption.
        schema.add_family_structural(name, MetricType::Summary, labels, &[("quantile", "")]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedSource;
    impl QuantileSource for FixedSource {
        fn read_summary(&self, quantiles: &[f64]) -> SummaryReading {
            SummaryReading {
                values: quantiles.iter().map(|q| q * 100.0).collect(),
                sum: 250.0,
                count: 5,
            }
        }
    }

    /// The public constructor (the third-party `QuantileSource` path) builds
    /// exactly the reading a source assembles field-by-field.
    #[test]
    fn reading_constructor_round_trips_with_a_source() {
        let read = FixedSource.read_summary(&[0.5, 0.99]);
        let constructed = SummaryReading::new(vec![50.0, 99.0], 250.0, 5);
        assert_eq!(read, constructed);
    }

    #[test]
    fn summary_renders_quantiles_sum_and_count() {
        let summary = Summary::new(FixedSource).with_quantiles([0.5, 0.99]);
        let mut values = MetricValues::new();
        summary.collect_metric("latency", &[("svc", "api")], &mut values);

        assert_eq!(
            values
                .samples()
                .iter()
                .filter(|s| s.name == "latency")
                .count(),
            2
        );
        assert!(
            values
                .samples()
                .iter()
                .filter(|s| s.name == "latency")
                .all(|s| s.labels.iter().any(|(k, _)| k == "quantile"))
        );
        assert!(
            values
                .samples()
                .iter()
                .any(|s| s.name == "latency_sum" && s.value.to_string() == "250")
        );
        assert!(
            values
                .samples()
                .iter()
                .any(|s| s.name == "latency_count" && s.value.to_string() == "5")
        );
    }

    #[test]
    fn summary_describes_a_summary_family_with_quantile_label() {
        let mut schema = MetricSchema::new();
        Summary::new(FixedSource).describe_metric("latency", &[("svc", "api")], &mut schema);
        let family = schema.family("latency").unwrap();
        assert_eq!(family.metric_type, MetricType::Summary);
        assert!(family.labels.iter().any(|l| l == "quantile"));
        assert!(schema.validate().is_ok(), "{:?}", schema.validate());
    }

    #[test]
    fn user_quantile_label_on_a_summary_is_reserved() {
        // A user/inherited `quantile` must not be erased by the structural
        // compose + type exemption: validate must see ReservedLabel.
        let mut schema = MetricSchema::new();
        Summary::new(FixedSource).describe_metric(
            "latency",
            &[("quantile", "0.99"), ("svc", "api")],
            &mut schema,
        );
        assert_eq!(
            schema.validate().unwrap_err(),
            vec![crate::schema::SchemaError::ReservedLabel {
                name: "latency".to_owned(),
                label: "quantile".to_owned(),
            }]
        );
        assert!(
            schema
                .family("latency")
                .unwrap()
                .labels
                .iter()
                .any(|l| l == "quantile")
        );
    }

    #[test]
    fn out_of_range_quantiles_are_dropped() {
        let summary = Summary::new(FixedSource).with_quantiles([0.5, 1.5, -0.1]);
        let mut values = MetricValues::new();
        summary.collect_metric("latency", &[], &mut values);
        assert_eq!(
            values
                .samples()
                .iter()
                .filter(|s| s.name == "latency")
                .count(),
            1
        );
    }
}
