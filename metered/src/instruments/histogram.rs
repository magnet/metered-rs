//! Backend-agnostic histogram traits.
//!
//! metered ships three histogram backends -- the classic
//! [`BucketHistogram`] (explicit `le`
//! bounds) and the two log-linear ones,
//! [`FixedExponentialHistogram`]
//! and [`DynamicExponentialHistogram`].
//! [`Histogram`] is the common interface over all three (record values,
//! read the cumulative `le` snapshot), so application or framework code can be
//! generic over which backend a metric uses. [`ExponentialHistogram`] adds the
//! native log-linear view ([`ExponentialSnapshot`]) the two exponential backends
//! also expose, for `vmrange` / protobuf sinks.
//!
//! These traits sit *beside* the concrete types (whose inherent `observe`
//! returns a bucket index for exemplar use); the trait methods are for
//! backend-agnostic callers.

use crate::bucket_histogram::{BucketHistogram, HistogramSnapshot};
use crate::exponential_histogram::{
    DynamicExponentialHistogram, ExponentialSnapshot, FixedExponentialHistogram,
};
use std::time::Duration;

/// A value distribution that records observations and exposes a cumulative
/// `le` ([`HistogramSnapshot`]) view -- implemented by every metered histogram
/// backend, so callers can be generic over the choice.
///
/// This is the trait; the concrete default backend is
/// [`BucketHistogram`].
pub trait Histogram {
    /// Records one observation, in the histogram's base unit (seconds for
    /// durations).
    fn observe(&self, value: f64);

    /// Records a duration observation, converting to seconds (`f64`).
    fn observe_duration(&self, value: Duration) {
        self.observe(value.as_secs_f64());
    }

    /// The current cumulative `le` snapshot.
    fn snapshot(&self) -> HistogramSnapshot;

    /// The sum of all observed values so far, in the base unit.
    fn sum(&self) -> f64;

    /// The total number of observations recorded so far.
    fn count(&self) -> u64;
}

/// A [`Histogram`] that also exposes a native log-linear
/// ([`ExponentialSnapshot`]) view, for `vmrange` / Prometheus-native sinks.
pub trait ExponentialHistogram: Histogram {
    /// The current resolution schema (`base = 2^(2^-schema)`).
    fn schema(&self) -> i32;

    /// The current sparse, non-cumulative exponential snapshot.
    fn exponential_snapshot(&self) -> ExponentialSnapshot;
}

impl Histogram for BucketHistogram {
    fn observe(&self, value: f64) {
        BucketHistogram::observe(self, value);
    }

    fn observe_duration(&self, value: Duration) {
        BucketHistogram::observe_duration(self, value);
    }

    fn snapshot(&self) -> HistogramSnapshot {
        BucketHistogram::snapshot(self)
    }

    fn sum(&self) -> f64 {
        BucketHistogram::sum(self)
    }

    fn count(&self) -> u64 {
        BucketHistogram::count(self)
    }
}

impl Histogram for FixedExponentialHistogram {
    fn observe(&self, value: f64) {
        FixedExponentialHistogram::observe(self, value);
    }

    fn observe_duration(&self, value: Duration) {
        FixedExponentialHistogram::observe_duration(self, value);
    }

    fn snapshot(&self) -> HistogramSnapshot {
        self.to_histogram_snapshot()
    }

    fn sum(&self) -> f64 {
        FixedExponentialHistogram::sum(self)
    }

    fn count(&self) -> u64 {
        FixedExponentialHistogram::count(self)
    }
}

impl ExponentialHistogram for FixedExponentialHistogram {
    fn schema(&self) -> i32 {
        FixedExponentialHistogram::schema(self)
    }

    fn exponential_snapshot(&self) -> ExponentialSnapshot {
        FixedExponentialHistogram::snapshot(self)
    }
}

impl Histogram for DynamicExponentialHistogram {
    fn observe(&self, value: f64) {
        DynamicExponentialHistogram::observe(self, value);
    }

    fn observe_duration(&self, value: Duration) {
        DynamicExponentialHistogram::observe_duration(self, value);
    }

    fn snapshot(&self) -> HistogramSnapshot {
        self.to_histogram_snapshot()
    }

    fn sum(&self) -> f64 {
        DynamicExponentialHistogram::sum(self)
    }

    fn count(&self) -> u64 {
        DynamicExponentialHistogram::count(self)
    }
}

impl ExponentialHistogram for DynamicExponentialHistogram {
    fn schema(&self) -> i32 {
        DynamicExponentialHistogram::schema(self)
    }

    fn exponential_snapshot(&self) -> ExponentialSnapshot {
        DynamicExponentialHistogram::snapshot(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket_histogram::Buckets;

    fn record_into(h: &dyn Histogram) {
        h.observe(0.012);
        h.observe(0.4);
        h.observe(1.5);
    }

    #[test]
    fn backends_are_swappable_behind_the_trait() {
        let bucket = BucketHistogram::new(Buckets::seconds_default());
        let fixed = FixedExponentialHistogram::new(0.001, 10.0, 4);
        let dynamic = DynamicExponentialHistogram::with_params(5, 256);

        for h in [
            &bucket as &dyn Histogram,
            &fixed as &dyn Histogram,
            &dynamic as &dyn Histogram,
        ] {
            record_into(h);
            assert_eq!(h.count(), 3);
            assert!((h.sum() - (0.012 + 0.4 + 1.5)).abs() < 1e-9);
            // The cumulative le view ends at +Inf == count.
            let snap = h.snapshot();
            assert_eq!(snap.buckets.last().unwrap().cumulative_count, 3);
        }
    }

    #[test]
    fn exponential_backends_expose_native_view() {
        let fixed = FixedExponentialHistogram::new(0.001, 10.0, 4);
        let dynamic = DynamicExponentialHistogram::with_params(5, 256);
        for h in [
            &fixed as &dyn ExponentialHistogram,
            &dynamic as &dyn ExponentialHistogram,
        ] {
            h.observe(0.05);
            h.observe(0.05);
            let snap = h.exponential_snapshot();
            assert_eq!(snap.schema, h.schema());
            assert_eq!(snap.count, 2);
            assert_eq!(snap.buckets.iter().map(|b| b.count).sum::<u64>(), 2);
        }
    }
}
