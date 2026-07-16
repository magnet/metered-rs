//! Exponential (log-linear) histograms: the OpenTelemetry "exponential" /
//! Prometheus "native" family, and VictoriaMetrics `vmrange`'s shape.
//!
//! Unlike [`crate::bucket_histogram`], the bucket boundaries are not chosen by
//! hand. They are implicit powers of `base = 2^(2^-schema)`: bucket index `i`
//! covers `[base^i, base^(i+1))`. A larger `schema` means a smaller `base`, so
//! finer relative resolution (and more buckets): `schema` 3 ≈ 9% per bucket,
//! 5 ≈ 2.2%, 8 ≈ 0.27%. This gives bounded *relative* error across a wide
//! dynamic range from one parameter, with no boundary list to maintain.
//!
//! This module provides two backends over the same bucket-index space:
//! [`FixedExponentialHistogram`], a dense fully lock-free backend over a bounded
//! range, and [`DynamicExponentialHistogram`], a sparse auto-rescaling backend.
//! Both share [`ExponentialSnapshot`].
//!
//! Values are recorded in the histogram's base unit (seconds for durations).
//! Non-positive values land in a dedicated zero bucket (exponential bucketing is
//! only defined for `value > 0`).

mod dynamic;
mod fixed;

pub use dynamic::DynamicExponentialHistogram;
pub use fixed::{FixedExponentialHistogram, FixedHistogramError, MAX_FIXED_BUCKETS};

use crate::bucket_histogram::{Bucket, Exemplar, HistogramSnapshot};

/// Sentinel returned by `observe` for a non-positive (zero-bucket) observation,
/// since real exponential bucket indices are finite.
pub const ZERO_BUCKET: i32 = i32::MIN;

/// The smallest supported schema (coarsest: `base = 2`).
pub const MIN_SCHEMA: i32 = 0;
/// The largest supported schema (finest). `2^MAX_SCHEMA` must stay well within
/// `f64` integer precision; 20 (`base ≈ 1.0000066`) is far more resolution than
/// any monitoring use needs.
pub const MAX_SCHEMA: i32 = 20;

/// `2^schema` as an `f64` (the per-octave subdivision count).
#[inline]
fn subdivisions(schema: i32) -> f64 {
    (1u64 << schema) as f64
}

/// The bucket index a positive `value` maps to at `schema`.
///
/// Bucket `i` covers `[base^i, base^(i+1))`, so this is
/// `floor(log2(value) * 2^schema)`. Callers must ensure `value > 0`.
#[inline]
pub fn index_of(value: f64, schema: i32) -> i32 {
    (value.log2() * subdivisions(schema)).floor() as i32
}

/// The (inclusive) lower bound of bucket `index` at `schema`: `base^index`.
#[inline]
pub fn lower_bound(index: i32, schema: i32) -> f64 {
    (index as f64 / subdivisions(schema)).exp2()
}

/// The (exclusive) upper bound of bucket `index` at `schema`: `base^(index+1)`.
#[inline]
pub fn upper_bound(index: i32, schema: i32) -> f64 {
    lower_bound(index + 1, schema)
}

/// One populated bucket of an [`ExponentialSnapshot`] (non-cumulative).
#[derive(Clone, Debug, PartialEq)]
pub struct ExponentialBucket {
    /// The bucket index (boundaries are `base^index .. base^(index+1)`).
    pub index: i32,
    /// Inclusive lower bound, in the histogram's base unit.
    pub lower: f64,
    /// Exclusive upper bound, in the histogram's base unit.
    pub upper: f64,
    /// Number of observations in this bucket (not cumulative).
    pub count: u64,
    /// The sampled exemplar for this bucket, if any (one per bucket).
    pub exemplar: Option<Exemplar>,
}

/// A point-in-time view of an exponential histogram.
///
/// Buckets are **sparse** (only populated ones) and **non-cumulative** -- the
/// natural shape for `vmrange` and Prometheus native exposition. Use
/// [`ExponentialSnapshot::to_histogram_snapshot`] for the cumulative `le`
/// ([`HistogramSnapshot`]) view consumed by the classic OpenMetrics encoder.
#[derive(Clone, Debug, PartialEq)]
pub struct ExponentialSnapshot {
    /// The resolution schema (`base = 2^(2^-schema)`).
    pub schema: i32,
    /// Count of non-positive observations (the zero bucket).
    pub zero_count: u64,
    /// Populated positive buckets, ascending by index.
    pub buckets: Vec<ExponentialBucket>,
    /// Count of observations above the representable range (the open tail).
    pub overflow_count: u64,
    /// Sum of all observed values, in the base unit.
    pub sum: f64,
    /// Total number of observations.
    pub count: u64,
}

impl ExponentialSnapshot {
    /// Assembles a snapshot from the schema, the zero/overflow counters, the
    /// sum, and an **ascending** iterator of populated `(index, count)` buckets.
    /// Computes each bucket's bounds and the running total. Shared by both
    /// backends so the assembly lives in one place.
    pub(super) fn from_buckets(
        schema: i32,
        zero_count: u64,
        overflow_count: u64,
        sum: f64,
        populated: impl IntoIterator<Item = (i32, u64, Option<Exemplar>)>,
    ) -> Self {
        let mut count = zero_count + overflow_count;
        let buckets = populated
            .into_iter()
            .map(|(index, bucket_count, exemplar)| {
                count += bucket_count;
                ExponentialBucket {
                    index,
                    lower: lower_bound(index, schema),
                    upper: upper_bound(index, schema),
                    count: bucket_count,
                    exemplar,
                }
            })
            .collect();
        ExponentialSnapshot {
            schema,
            zero_count,
            buckets,
            overflow_count,
            sum,
            count,
        }
    }

    /// Converts to the cumulative `le` [`HistogramSnapshot`] used by the classic
    /// OpenMetrics encoder.
    ///
    /// Lossless aggregation: each populated exponential bucket becomes one `le`
    /// bucket at its upper bound, with counts accumulated; non-positive
    /// observations fold into the cumulative base, and the open tail into the
    /// final `+Inf` bucket. The emitted `le` values are the exponential
    /// boundaries (machine numbers), not human-round numbers.
    pub fn to_histogram_snapshot(&self) -> HistogramSnapshot {
        let mut buckets = Vec::with_capacity(self.buckets.len() + 1);
        let mut cumulative = self.zero_count;
        for bucket in &self.buckets {
            cumulative += bucket.count;
            buckets.push(Bucket {
                le: bucket.upper,
                cumulative_count: cumulative,
                exemplar: bucket.exemplar.clone(),
            });
        }
        buckets.push(Bucket {
            le: f64::INFINITY,
            cumulative_count: self.count,
            exemplar: None,
        });
        HistogramSnapshot {
            buckets,
            sum: self.sum,
            count: self.count,
        }
    }

    /// Returns a compact, human-readable diagnostic dump of the sparse
    /// exponential buckets.
    ///
    /// This is intended for out-of-band troubleshooting (debug endpoints,
    /// support bundles, agent investigations). Scrape sinks should keep using
    /// structured snapshots and their normal exposition formats.
    pub fn diagnostic_text(&self) -> String {
        use std::fmt::Write;

        let mut out = String::new();
        let _ = writeln!(
            out,
            "schema={} count={} sum={} zero_count={} overflow_count={}",
            self.schema, self.count, self.sum, self.zero_count, self.overflow_count
        );
        for bucket in &self.buckets {
            let _ = writeln!(
                out,
                "bucket index={} range=[{:.6e}, {:.6e}) count={}",
                bucket.index, bucket.lower, bucket.upper, bucket.count
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_bounds_are_consistent_across_schemas() {
        assert_eq!(index_of(1.0, 0), 0);
        assert_eq!(index_of(1.5, 0), 0);
        assert_eq!(index_of(2.0, 0), 1);
        assert_eq!(index_of(3.0, 0), 1);
        assert_eq!(index_of(4.0, 0), 2);
        assert_eq!(lower_bound(0, 0), 1.0);
        assert_eq!(upper_bound(0, 0), 2.0);
        assert_eq!(lower_bound(1, 0), 2.0);

        for &(value, schema) in &[(0.012, 3), (0.4, 5), (1e-6, 2), (12.5, 4)] {
            let i = index_of(value, schema);
            assert!(
                lower_bound(i, schema) <= value && value < upper_bound(i, schema),
                "value {} not in bucket {} at schema {}",
                value,
                i,
                schema
            );
        }
    }

    #[test]
    fn observe_routes_values_sum_and_count() {
        let h = FixedExponentialHistogram::new(0.001, 10.0, 3);
        h.observe(0.012);
        h.observe(0.4);
        h.observe(0.4);
        h.observe(0.0);
        h.observe(100.0);

        assert_eq!(h.count(), 5);
        assert!((h.sum() - (0.012 + 0.4 + 0.4 + 0.0 + 100.0)).abs() < 1e-9);

        let snap = h.snapshot();
        assert_eq!(snap.zero_count, 1);
        assert_eq!(snap.overflow_count, 1);
        let four_tenths = snap
            .buckets
            .iter()
            .find(|b| b.lower <= 0.4 && 0.4 < b.upper)
            .expect("populated bucket for 0.4");
        assert_eq!(four_tenths.count, 2);
        assert!(snap.buckets.windows(2).all(|w| w[0].index < w[1].index));
    }

    #[test]
    fn fixed_drops_nan_without_poisoning_sum_or_count() {
        let h = FixedExponentialHistogram::new(0.001, 10.0, 3);
        h.observe(0.4);
        h.observe(f64::NAN);
        h.observe(0.4);

        // The NaN is dropped entirely: `sum` stays finite and correct, and it is
        // not counted in any bucket.
        assert!(h.sum().is_finite());
        assert!((h.sum() - 0.8).abs() < 1e-9);
        assert_eq!(h.count(), 2);
        assert_eq!(h.snapshot().zero_count, 0);
    }

    #[test]
    fn dynamic_drops_nan_without_poisoning_sum_or_count() {
        let h = DynamicExponentialHistogram::with_params(5, 256);
        h.observe(0.4);
        h.observe(f64::NAN);
        h.observe(0.4);

        assert!(h.sum().is_finite());
        assert!((h.sum() - 0.8).abs() < 1e-9);
        assert_eq!(h.count(), 2);
        assert_eq!(h.snapshot().zero_count, 0);
    }

    #[test]
    fn le_conversion_is_cumulative_monotonic_and_totals_match() {
        let h = FixedExponentialHistogram::new(0.001, 10.0, 3);
        for v in [0.002, 0.05, 0.05, 0.4, 3.0] {
            h.observe(v);
        }
        let le = h.to_histogram_snapshot();
        assert!(le
            .buckets
            .windows(2)
            .all(|w| w[0].cumulative_count <= w[1].cumulative_count));
        let inf = le.buckets.last().unwrap();
        assert!(inf.le.is_infinite());
        assert_eq!(inf.cumulative_count, h.count());
        assert_eq!(le.count, 5);
    }

    #[test]
    fn relative_resolution_tightens_with_schema() {
        for schema in [2, 4, 6] {
            let base = upper_bound(0, schema) / lower_bound(0, schema);
            let expected = 2f64.powf(1.0 / subdivisions(schema));
            assert!((base - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn diagnostic_text_includes_summary_and_sparse_buckets() {
        let h = FixedExponentialHistogram::new(0.001, 10.0, 3);
        h.observe(0.05);
        h.observe(0.4);
        let text = h.snapshot().diagnostic_text();
        assert!(text.contains("schema=3 count=2"));
        assert!(text.contains("bucket index="));
        assert!(text.contains("range=["));
    }

    #[test]
    fn dynamic_adopts_first_exemplar_and_upgrades_on_interest() {
        use crate::bucket_histogram::Exemplar;
        let exemplar = |id: &str| Exemplar {
            labels: vec![("trace_id".to_owned(), id.to_owned())],
            value: 0.0,
            timestamp_seconds: None,
        };

        let h = DynamicExponentialHistogram::with_params(5, 256);
        // First offer in the bucket this window is adopted.
        assert!(h.observe_with_exemplar(0.05, exemplar("first"), false));
        // A later non-interesting offer does not replace it.
        assert!(!h.observe_with_exemplar(0.05, exemplar("second"), false));
        // An interesting offer upgrades the standing non-interesting sample, once.
        assert!(h.observe_with_exemplar(0.05, exemplar("error"), true));
        // A second interesting offer does not re-adopt (already interesting).
        assert!(!h.observe_with_exemplar(0.05, exemplar("error2"), true));

        let snap = h.snapshot();
        let bucket = snap
            .buckets
            .iter()
            .find(|b| b.lower <= 0.05 && 0.05 < b.upper)
            .expect("populated bucket for 0.05");
        assert_eq!(bucket.count, 4);
        assert_eq!(bucket.exemplar.as_ref().unwrap().labels[0].1, "error");
        // The sample survives the cumulative `le` conversion.
        let le = snap.to_histogram_snapshot();
        assert!(le
            .buckets
            .iter()
            .any(|b| b.exemplar.as_ref().map(|e| e.labels[0].1.as_str()) == Some("error")));
    }

    #[test]
    fn dynamic_housekeep_reopens_exemplar_window() {
        use crate::bucket_histogram::Exemplar;
        let exemplar = |id: &str| Exemplar {
            labels: vec![("trace_id".to_owned(), id.to_owned())],
            value: 0.0,
            timestamp_seconds: None,
        };
        let standing = |h: &DynamicExponentialHistogram| {
            h.snapshot()
                .buckets
                .iter()
                .find(|b| b.lower <= 0.05 && 0.05 < b.upper)
                .and_then(|b| b.exemplar.as_ref())
                .map(|e| e.labels[0].1.clone())
                .expect("populated bucket with exemplar")
        };

        let h = DynamicExponentialHistogram::with_params(5, 256);
        assert!(h.observe_with_exemplar(0.05, exemplar("window1"), false));
        // The first adoption puts the histogram into exemplar-window
        // maintenance mode: every scrape should reopen windows from now on.
        assert!(h.needs_housekeep());

        // Housekeep reopens the window but preserves the standing value for the
        // scrape that immediately follows it.
        h.housekeep();
        assert!(
            h.needs_housekeep(),
            "exemplar-window maintenance stays enabled after the first exemplar"
        );
        assert_eq!(standing(&h), "window1");

        // The next offer after housekeep adopts a fresh sample.
        assert!(h.observe_with_exemplar(0.05, exemplar("window2"), false));
        assert_eq!(standing(&h), "window2");
    }

    #[test]
    fn dynamic_records_and_snapshots() {
        let h = DynamicExponentialHistogram::with_params(5, 256);
        for v in [0.002, 0.05, 0.05, 0.4, 3.0] {
            h.observe(v);
        }
        h.observe(0.0);
        assert_eq!(h.count(), 6);
        assert!((h.sum() - (0.002 + 0.05 + 0.05 + 0.4 + 3.0)).abs() < 1e-9);

        let snap = h.snapshot();
        assert_eq!(snap.zero_count, 1);
        assert_eq!(snap.count, 6);
        assert!(snap.buckets.windows(2).all(|w| w[0].index < w[1].index));

        let le = h.to_histogram_snapshot();
        assert_eq!(le.count, 6);
        assert!(le
            .buckets
            .windows(2)
            .all(|w| w[0].cumulative_count <= w[1].cumulative_count));
    }

    #[test]
    fn dynamic_downscales_under_wide_spread_without_losing_observations() {
        let h = DynamicExponentialHistogram::with_params(8, 16);
        let start_schema = h.schema();

        let mut total = 0u64;
        for k in 0..2000 {
            let v = 1e-6 * 1.05f64.powi(k % 320);
            h.observe(v);
            total += 1;
            if h.needs_rescale() {
                h.rescale_if_needed();
            }
        }
        h.rescale_if_needed();

        assert!(
            h.schema() < start_schema,
            "wide spread should have downscaled the schema (from {start_schema} to {})",
            h.schema()
        );
        assert_eq!(h.count(), total);
        assert_eq!(h.snapshot().count, total);
    }

    #[test]
    fn dynamic_is_lock_free_and_consistent_under_concurrent_observe_and_housekeep() {
        use std::sync::Arc;
        use std::thread;

        let h = Arc::new(DynamicExponentialHistogram::with_params(6, 64));
        let threads = 4;
        let per_thread = 5_000u64;

        let mut handles = Vec::new();
        for t in 0..threads {
            let h = Arc::clone(&h);
            handles.push(thread::spawn(move || {
                for k in 0..per_thread {
                    let v = 1e-5 * 1.03f64.powi(((k + t as u64 * 97) % 400) as i32);
                    h.observe(v);
                }
            }));
        }
        let maintainer = {
            let h = Arc::clone(&h);
            thread::spawn(move || {
                for _ in 0..2_000 {
                    h.housekeep();
                    std::thread::yield_now();
                }
            })
        };

        for handle in handles {
            handle.join().unwrap();
        }
        maintainer.join().unwrap();
        h.housekeep();

        assert_eq!(h.count(), threads as u64 * per_thread);
    }

    #[test]
    fn dynamic_concurrent_exemplars_and_housekeep_stay_consistent() {
        use crate::bucket_histogram::Exemplar;
        use std::sync::Arc;
        use std::thread;

        // Hammer `observe_with_exemplar` (mixed interesting/non-interesting, so the
        // adoption + upgrade CAS and the per-window state are exercised) while a
        // maintainer drives `housekeep` (rescale + the `adopted`-gated `reopen_all`).
        // The invariant under all this churn: every observation is counted exactly
        // once, and the snapshot stays well-formed. Guards the `adopted`/`reopen_all`
        // ordering path where a real bug was previously found.
        let h = Arc::new(DynamicExponentialHistogram::with_params(6, 64));
        let threads = 4;
        let per_thread = 5_000u64;

        let mut handles = Vec::new();
        for t in 0..threads {
            let h = Arc::clone(&h);
            handles.push(thread::spawn(move || {
                for k in 0..per_thread {
                    let v = 1e-4 * 1.05f64.powi(((k + t as u64 * 53) % 200) as i32);
                    let ex = Exemplar {
                        labels: vec![("trace_id".to_owned(), format!("{t}-{k}"))],
                        value: 0.0,
                        timestamp_seconds: None,
                    };
                    // 1-in-7 offers are "interesting" (the upgrade path).
                    h.observe_with_exemplar(v, ex, k % 7 == 0);
                }
            }));
        }
        let maintainer = {
            let h = Arc::clone(&h);
            thread::spawn(move || {
                for _ in 0..2_000 {
                    h.housekeep();
                    std::thread::yield_now();
                }
            })
        };

        for handle in handles {
            handle.join().unwrap();
        }
        maintainer.join().unwrap();
        h.housekeep();

        assert_eq!(h.count(), threads as u64 * per_thread);
        assert_eq!(h.snapshot().count, threads as u64 * per_thread);
    }
}
