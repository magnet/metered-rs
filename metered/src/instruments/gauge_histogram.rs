//! First-class OpenMetrics **gauge histogram** support, built symmetrically to
//! the other metered metrics: a read-seam trait ([`GaugeHistogramSource`]), a
//! renderer ([`GaugeHistogram`]), and a concrete instrument ([`GaugeBuckets`]).
//!
//! Unlike a cumulative histogram, a gauge histogram's bucket counts reflect a
//! *current* population and may go **down** (e.g. the size distribution of items
//! currently held). It renders `name_bucket{le="…"}` (cumulative), `name_gcount`
//! and `name_gsum`. Write your own adapter by implementing
//! [`GaugeHistogramSource`].

use crate::labels::slices::with_labels;
use crate::metric_tree::{Metric, MetricType};
use crate::schema::MetricSchema;
use crate::values::MetricValues;
use std::sync::atomic::{AtomicI64, Ordering};

/// One cumulative bucket of a [`GaugeHistogramSnapshot`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GaugeBucket {
    /// Inclusive upper bound (`+Inf` for the overflow bucket).
    pub le: f64,
    /// Current cumulative count at or below `le` (may decrease over time).
    pub cumulative_count: i64,
}

/// A point-in-time reading of a gauge histogram.
#[derive(Clone, Debug, PartialEq)]
pub struct GaugeHistogramSnapshot {
    /// Cumulative buckets in ascending `le` order, ending at `+Inf`.
    pub buckets: Vec<GaugeBucket>,
    /// Current sum of observed values.
    pub gsum: f64,
    /// Current total count (the `+Inf` bucket's cumulative count).
    pub gcount: i64,
}

/// A read-side source of a gauge histogram. Implement this to expose your own
/// current-value distribution as a [`GaugeHistogram`].
pub trait GaugeHistogramSource {
    /// Reads the current distribution.
    fn snapshot(&self) -> GaugeHistogramSnapshot;
}

/// Renders any [`GaugeHistogramSource`] as an OpenMetrics gauge histogram.
#[derive(Debug, Clone)]
pub struct GaugeHistogram<S> {
    source: S,
}

impl<S> GaugeHistogram<S> {
    /// A gauge histogram over `source`.
    pub fn new(source: S) -> Self {
        GaugeHistogram { source }
    }

    /// The underlying source.
    pub fn source(&self) -> &S {
        &self.source
    }
}

impl<S: GaugeHistogramSource> Metric for GaugeHistogram<S> {
    fn metric_type(&self) -> MetricType {
        MetricType::GaugeHistogram
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let snapshot = self.source.snapshot();
        debug_assert_eq!(
            snapshot.buckets.last().map(|b| b.cumulative_count),
            Some(snapshot.gcount),
            "GaugeHistogramSource::snapshot must report gcount equal to the +Inf bucket's cumulative count"
        );
        let bucket_name = format!("{name}_bucket");
        for bucket in &snapshot.buckets {
            let le = if bucket.le.is_infinite() {
                "+Inf".to_owned()
            } else {
                bucket.le.to_string()
            };
            let bucket_labels = with_labels(labels, [("le", le.as_str())]);
            values.sample(&bucket_name, &bucket_labels, bucket.cumulative_count);
        }
        values.sample(&format!("{name}_gcount"), labels, snapshot.gcount);
        values.sample(&format!("{name}_gsum"), labels, snapshot.gsum);
    }

    fn describe_metric(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        let all = with_labels(labels, [("le", "")]);
        schema.add_family(name, MetricType::GaugeHistogram, &all);
    }
}

/// A concrete gauge histogram: fixed `le` boundaries with per-bucket up/down
/// counters. Track a current population by calling [`enter`](GaugeBuckets::enter)
/// when a value appears and [`leave`](GaugeBuckets::leave) when it goes away.
///
/// # Contract
///
/// Always call [`leave`](GaugeBuckets::leave) with the **same** value you
/// [`enter`](GaugeBuckets::enter)-ed, so the decrement lands in the same bucket
/// as the increment, and keep net enters ≥ net leaves -- otherwise the bucket
/// counts (and the derived `gcount`) can go negative.
///
/// A [`snapshot`](GaugeHistogramSource::snapshot) is a lock-free,
/// eventually-consistent read: the bucket counts, `gsum`, and the derived
/// `gcount` are read independently, so a snapshot taken during concurrent
/// `enter`/`leave` may be momentarily inconsistent. This is the standard
/// trade-off for lock-free monitoring instruments and is acceptable here.
#[derive(Debug)]
pub struct GaugeBuckets {
    /// Finite upper bounds in ascending order; an implicit `+Inf` bucket follows.
    bounds: Vec<f64>,
    /// Per-bucket current (non-cumulative) counts; `counts[bounds.len()]` is `+Inf`.
    counts: Vec<AtomicI64>,
    /// Current sum of observed values. Unlike `gcount` it is not derivable from
    /// the buckets, so it is tracked directly.
    gsum: crate::instruments::atomic_f64::AtomicF64,
}

impl GaugeBuckets {
    /// Builds a gauge histogram with the given finite upper bounds.
    ///
    /// Bounds are normalized: sorted ascending, de-duplicated, and any
    /// non-finite value (`NaN`, `±∞`) is dropped -- the `+Inf` overflow bucket
    /// is always implicit. Zero and negative bounds are kept, since a gauge
    /// histogram may measure non-positive quantities.
    pub fn new(bounds: impl IntoIterator<Item = f64>) -> Self {
        let mut bounds: Vec<f64> = bounds.into_iter().filter(|b| b.is_finite()).collect();
        bounds.sort_by(f64::total_cmp);
        bounds.dedup();
        let counts = (0..=bounds.len()).map(|_| AtomicI64::new(0)).collect();
        GaugeBuckets {
            bounds,
            counts,
            gsum: crate::instruments::atomic_f64::AtomicF64::new(0.0),
        }
    }

    fn index_of(&self, value: f64) -> usize {
        // First bound `>= value` (inclusive-`le` semantics): `partition_point`
        // counts the bounds strictly less than `value`, which is exactly that
        // index. If `value` exceeds every bound the index is `bounds.len()` --
        // the implicit `+Inf` bucket.
        self.bounds.partition_point(|&b| b < value)
    }

    /// Records that a `value` is now present.
    pub fn enter(&self, value: f64) {
        self.counts[self.index_of(value)].fetch_add(1, Ordering::Relaxed);
        self.gsum.add(value);
    }

    /// Records that a previously-present `value` is gone.
    ///
    /// Pass the **same** `value` that was [`enter`](GaugeBuckets::enter)-ed, so
    /// the decrement lands in the same bucket the increment did; keep net enters
    /// ≥ net leaves or the counts go negative.
    pub fn leave(&self, value: f64) {
        self.counts[self.index_of(value)].fetch_sub(1, Ordering::Relaxed);
        self.gsum.add(-value);
    }
}

impl GaugeHistogramSource for GaugeBuckets {
    fn snapshot(&self) -> GaugeHistogramSnapshot {
        let mut cumulative = 0i64;
        let mut buckets = Vec::with_capacity(self.bounds.len() + 1);
        for (i, &le) in self.bounds.iter().enumerate() {
            cumulative += self.counts[i].load(Ordering::Relaxed);
            buckets.push(GaugeBucket {
                le,
                cumulative_count: cumulative,
            });
        }
        cumulative += self.counts[self.bounds.len()].load(Ordering::Relaxed);
        buckets.push(GaugeBucket {
            le: f64::INFINITY,
            cumulative_count: cumulative,
        });
        // `gcount` is exactly the `+Inf` cumulative total; deriving it from the
        // same running sum means it can never disagree with the last bucket
        // (no torn read between a separate counter and the buckets).
        GaugeHistogramSnapshot {
            buckets,
            gsum: self.gsum.get(),
            gcount: cumulative,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enter_and_leave_track_a_current_distribution() {
        let hist = GaugeBuckets::new([1.0, 2.0]);
        hist.enter(0.5);
        hist.enter(1.5);
        hist.enter(1.5);
        hist.leave(1.5);
        let snap = hist.snapshot();
        assert_eq!(snap.gcount, 2);
        assert_eq!(snap.gsum, 2.0);
        assert_eq!(
            snap.buckets[0],
            GaugeBucket {
                le: 1.0,
                cumulative_count: 1
            }
        );
        assert_eq!(
            snap.buckets[1],
            GaugeBucket {
                le: 2.0,
                cumulative_count: 2
            }
        );
        assert!(snap.buckets[2].le.is_infinite() && snap.buckets[2].cumulative_count == 2);
    }

    #[test]
    fn new_normalizes_unsorted_and_duplicate_bounds() {
        let hist = GaugeBuckets::new([2.0, 1.0, 2.0, f64::NAN]);
        let snap = hist.snapshot();
        let finite_les: Vec<f64> = snap
            .buckets
            .iter()
            .map(|bucket| bucket.le)
            .filter(|le| le.is_finite())
            .collect();
        assert_eq!(finite_les, vec![1.0, 2.0]);
        assert!(snap.buckets.last().unwrap().le.is_infinite());
    }

    #[test]
    fn renders_buckets_gcount_and_gsum() {
        let hist = GaugeBuckets::new([1.0, 2.0]);
        hist.enter(0.5);
        hist.enter(1.5);
        let gh = GaugeHistogram::new(hist);

        let mut values = MetricValues::new();
        gh.collect_metric("queue_size", &[], &mut values);
        let mut schema = MetricSchema::new();
        gh.describe_metric("queue_size", &[], &mut schema);

        assert_eq!(
            schema.family("queue_size").unwrap().metric_type,
            MetricType::GaugeHistogram
        );
        assert!(values
            .samples()
            .iter()
            .any(|s| s.name == "queue_size_gcount" && s.value.to_string() == "2"));
        assert!(values
            .samples()
            .iter()
            .any(|s| s.name == "queue_size_gsum" && s.value.to_string() == "2"));
        assert!(values
            .samples()
            .iter()
            .any(|s| s.name == "queue_size_bucket"
                && s.labels.iter().any(|(k, v)| k == "le" && v == "+Inf")));
    }
}
