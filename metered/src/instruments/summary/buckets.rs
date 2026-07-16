//! [`QuantileSource`](super::QuantileSource) over any [`Histogram`], plus the
//! shared quantile interpolation used here and by [`migration`](crate::migration).

use super::QuantileSource;
use crate::Histogram;

/// Estimates the value at quantile `q` from cumulative `(le, cumulative_count)`
/// buckets, using Prometheus' `histogram_quantile()` interpolation. `NaN` when
/// there are no observations.
#[doc(hidden)]
pub fn quantile_from_buckets(q: f64, buckets: &[(f64, u64)], total: u64) -> f64 {
    if total == 0 || buckets.is_empty() {
        return f64::NAN;
    }
    if q <= 0.0 {
        // The lowest finite bound is the best lower estimate.
        return buckets.first().map(|&(le, _)| le).unwrap_or(f64::NAN);
    }
    let rank = q * total as f64;
    let index = buckets
        .iter()
        .position(|&(_, cumulative)| cumulative as f64 >= rank)
        .unwrap_or(buckets.len() - 1);
    // Rank falls in (or past) the open-ended +Inf bucket: the largest finite
    // bound is the best estimate.
    if buckets[index].0.is_infinite() {
        return buckets
            .iter()
            .rev()
            .map(|&(le, _)| le)
            .find(|le| le.is_finite())
            .unwrap_or(f64::NAN);
    }
    let bucket_end = buckets[index].0;
    let (bucket_start, cumulative_before) = if index == 0 {
        (0.0, 0)
    } else {
        (buckets[index - 1].0, buckets[index - 1].1)
    };
    let count_in_bucket = buckets[index].1 - cumulative_before;
    if count_in_bucket == 0 {
        return bucket_end;
    }
    let rank_in_bucket = rank - cumulative_before as f64;
    bucket_start + (bucket_end - bucket_start) * (rank_in_bucket / count_in_bucket as f64)
}

/// Exposes any [`Histogram`] as a [`QuantileSource`], so a histogram can also be
/// rendered as a [`Summary`](super::Summary). Quantiles are bounded by the
/// histogram's bucket resolution (for the exponential histogram, bounded
/// relative error).
pub struct BucketQuantiles<'a, H: Histogram> {
    source: &'a H,
}

impl<'a, H: Histogram> BucketQuantiles<'a, H> {
    /// Borrows `source` for summary derivation.
    pub fn new(source: &'a H) -> Self {
        BucketQuantiles { source }
    }
}

impl<H: Histogram> QuantileSource for BucketQuantiles<'_, H> {
    fn read_summary(&self, quantiles: &[f64]) -> crate::summary::SummaryReading {
        let snapshot = self.source.snapshot();
        let buckets: Vec<(f64, u64)> = snapshot
            .buckets
            .iter()
            .map(|b| (b.le, b.cumulative_count))
            .collect();
        crate::summary::SummaryReading {
            values: quantiles
                .iter()
                .map(|&q| quantile_from_buckets(q, &buckets, snapshot.count))
                .collect(),
            sum: snapshot.sum,
            count: snapshot.count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket_histogram::Buckets;
    use crate::metric_tree::Metric;
    use crate::summary::Summary;
    use crate::values::MetricValues;
    use crate::BucketHistogram;

    #[test]
    fn bucket_histogram_renders_as_a_summary() {
        let histogram = BucketHistogram::new(Buckets::custom([1.0, 2.0, 3.0, 4.0]));
        for v in [0.5, 1.5, 1.5, 2.5, 3.5] {
            histogram.observe(v);
        }
        let summary = Summary::new(BucketQuantiles::new(&histogram));
        let mut values = MetricValues::new();
        summary.collect_metric("response_time", &[], &mut values);

        assert_eq!(
            values
                .samples()
                .iter()
                .find(|s| s.name == "response_time_count")
                .unwrap()
                .value
                .to_string(),
            "5"
        );
        assert!(values.samples().iter().any(|s| s.name == "response_time"));
    }

    #[test]
    fn quantile_interpolates_within_landing_bucket() {
        let buckets = vec![(1.0, 0), (2.0, 10), (3.0, 10), (f64::INFINITY, 10)];
        assert!((quantile_from_buckets(0.5, &buckets, 10) - 1.5).abs() < 1e-9);
    }

    #[test]
    fn quantile_in_inf_bucket_returns_largest_finite_bound() {
        let buckets = vec![(1.0, 0), (2.0, 0), (f64::INFINITY, 5)];
        assert_eq!(quantile_from_buckets(0.99, &buckets, 5), 2.0);
    }

    #[test]
    fn empty_histogram_quantile_is_nan() {
        let buckets = vec![(1.0, 0), (f64::INFINITY, 0)];
        assert!(quantile_from_buckets(0.5, &buckets, 0).is_nan());
    }

    #[test]
    fn exponential_histogram_is_a_quantile_source() {
        use crate::exponential_histogram::DynamicExponentialHistogram;
        let hist = DynamicExponentialHistogram::default();
        for i in 1..=1000u64 {
            hist.observe(i as f64);
        }
        let qs = BucketQuantiles::new(&hist);
        let r = qs.read_summary(&[0.5]);
        assert_eq!(r.count, 1000);
        // p50 of 1..=1000 is ~500 within the exponential histogram's relative error.
        assert!((r.values[0] - 500.0).abs() <= 0.05 * 500.0);
    }

    #[test]
    fn bucket_quantiles_read_summary_is_consistent() {
        let histogram = BucketHistogram::new(Buckets::custom([1.0, 2.0, 3.0, 4.0]));
        for v in [0.5, 1.5, 1.5, 2.5, 3.5] {
            histogram.observe(v);
        }
        let qs = BucketQuantiles::new(&histogram);
        let reading = qs.read_summary(&[0.5, 0.9]);
        assert_eq!(reading.count, 5);
        assert_eq!(reading.values.len(), 2);

        // The reported values match `quantile_from_buckets` over the same
        // (quiescent) snapshot, and are monotonic in the (sorted) input.
        let snapshot = histogram.snapshot();
        let expected_buckets: Vec<(f64, u64)> = snapshot
            .buckets
            .iter()
            .map(|b| (b.le, b.cumulative_count))
            .collect();
        assert_eq!(
            reading.values[0],
            quantile_from_buckets(0.5, &expected_buckets, snapshot.count)
        );
        assert_eq!(
            reading.values[1],
            quantile_from_buckets(0.9, &expected_buckets, snapshot.count)
        );
        assert!(reading.values[0] <= reading.values[1]);
        assert_eq!(reading.sum, snapshot.sum);
    }
}
