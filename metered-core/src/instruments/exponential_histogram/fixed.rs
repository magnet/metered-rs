use super::{ExponentialSnapshot, MAX_SCHEMA, MIN_SCHEMA, ZERO_BUCKET, index_of};
use crate::bucket_histogram::HistogramSnapshot;
use crate::instruments::atomic_f64::AtomicF64;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Hard ceiling on the dense bucket span a [`FixedExponentialHistogram`] will
/// allocate. A wide range at a fine schema can otherwise request billions of
/// buckets (`[1e-9, 1e9]` at schema 20 is ~60 GB). ~1M buckets (8 MB) is far
/// more than any legitimate fixed histogram needs -- the dynamic backend is
/// the tool for genuinely wide or unknown ranges. A request past the ceiling
/// is rejected by [`FixedExponentialHistogram::try_new`], not silently capped.
pub const MAX_FIXED_BUCKETS: usize = 1 << 20;

/// Why a [`FixedExponentialHistogram::try_new`] request was rejected.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum FixedHistogramError {
    /// Bounds were not finite with `0 < min <= max`.
    InvalidBounds {
        /// The requested lower bound.
        min: f64,
        /// The requested upper bound.
        max: f64,
    },
    /// `schema` was outside `[MIN_SCHEMA, MAX_SCHEMA]`. Reported rather than
    /// silently clamped, so the caller's resolution expectation is not
    /// quietly rewritten.
    SchemaOutOfRange {
        /// The rejected schema.
        schema: i32,
    },
    /// The `[min, max]` range at `schema` needs more than
    /// [`MAX_FIXED_BUCKETS`] dense buckets. Use a coarser schema, a narrower
    /// range, or [`DynamicExponentialHistogram`](super::DynamicExponentialHistogram).
    TooManyBuckets {
        /// The bucket count the request would have allocated.
        buckets: u64,
    },
}

impl fmt::Display for FixedHistogramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FixedHistogramError::InvalidBounds { min, max } => write!(
                f,
                "FixedExponentialHistogram requires finite 0 < min <= max, got min={min} max={max}"
            ),
            FixedHistogramError::SchemaOutOfRange { schema } => write!(
                f,
                "schema {schema} is outside the supported range [{MIN_SCHEMA}, {MAX_SCHEMA}]"
            ),
            FixedHistogramError::TooManyBuckets { buckets } => write!(
                f,
                "range/schema needs {buckets} dense buckets, over the {MAX_FIXED_BUCKETS} ceiling"
            ),
        }
    }
}

impl std::error::Error for FixedHistogramError {}

/// A dense, fixed-schema exponential histogram.
///
/// Records into a preallocated `[AtomicU64]` spanning a bounded index range at a
/// fixed `schema`. The observe path is fully lock-free and allocation-free: one
/// `log2`-based index plus one `fetch_add`. Values below the range fold into the
/// lowest bucket; values above it into an overflow (open-tail) counter;
/// non-positive values into a zero bucket.
///
/// Memory is `O(range)` and fixed regardless of traffic, so prefer a modest
/// `schema` for wide ranges (see the module docs). For sparse/unbounded ranges
/// or very high resolution, prefer the dynamic backend.
#[derive(Debug)]
pub struct FixedExponentialHistogram {
    schema: i32,
    min_index: i32,
    counts: Box<[AtomicU64]>,
    zero_count: AtomicU64,
    overflow_count: AtomicU64,
    sum: AtomicF64,
}

impl FixedExponentialHistogram {
    /// Builds a histogram covering `[min, max]` (base unit) at `schema`, or
    /// reports why the request is out of contract.
    ///
    /// `min`/`max` must be finite with `0 < min <= max`; `schema` must be in
    /// `[MIN_SCHEMA, MAX_SCHEMA]` (it is **not** silently clamped); and the
    /// dense span must fit within [`MAX_FIXED_BUCKETS`]. The representable
    /// range is rounded out to the enclosing bucket boundaries.
    pub fn try_new(min: f64, max: f64, schema: i32) -> Result<Self, FixedHistogramError> {
        if !(MIN_SCHEMA..=MAX_SCHEMA).contains(&schema) {
            return Err(FixedHistogramError::SchemaOutOfRange { schema });
        }
        if !(min.is_finite() && max.is_finite() && min > 0.0 && max >= min) {
            return Err(FixedHistogramError::InvalidBounds { min, max });
        }
        let min_index = index_of(min, schema);
        let max_index = index_of(max, schema);
        // Compute the span in i64 -- a wide range at a fine schema can exceed
        // i32 -- and reject a request past the allocation ceiling instead of
        // quietly degrading its top-end resolution.
        let requested = (i64::from(max_index) - i64::from(min_index) + 1).max(1) as u64;
        if requested > MAX_FIXED_BUCKETS as u64 {
            return Err(FixedHistogramError::TooManyBuckets { buckets: requested });
        }
        let counts = (0..requested)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Ok(FixedExponentialHistogram {
            schema,
            min_index,
            counts,
            zero_count: AtomicU64::new(0),
            overflow_count: AtomicU64::new(0),
            sum: AtomicF64::new(0.0),
        })
    }

    /// Builds a histogram covering `[min, max]` (base unit) at `schema`,
    /// panicking on an out-of-contract request. Prefer
    /// [`try_new`](FixedExponentialHistogram::try_new) when the bounds or
    /// schema are not known-good constants.
    #[must_use]
    pub fn new(min: f64, max: f64, schema: i32) -> Self {
        FixedExponentialHistogram::try_new(min, max, schema)
            .expect("FixedExponentialHistogram::new called with an out-of-contract range or schema")
    }

    /// A sensible default for sub-microsecond to multi-second latencies, in
    /// seconds: `[1µs, 30s]` at `schema` 3 (~9% relative resolution, ~200
    /// buckets, ~1.6 KB).
    #[must_use]
    pub fn seconds_default() -> Self {
        FixedExponentialHistogram::new(0.000_001, 30.0, 3)
    }

    /// The resolution schema.
    #[must_use]
    pub fn schema(&self) -> i32 {
        self.schema
    }

    /// Records one observation, in the histogram's base unit. Returns the
    /// exponential bucket index the value mapped to (or [`ZERO_BUCKET`] for a
    /// non-positive value), so an exemplar layer can decide cheaply -- a high
    /// index means a slow/large outlier -- without a second lookup.
    pub fn observe(&self, value: f64) -> i32 {
        // A NaN observation is dropped before it can touch `sum` (one NaN would
        // poison `_sum` forever) or any count; non-positive finite values still
        // fold into the zero bucket below.
        if value.is_nan() {
            return ZERO_BUCKET;
        }
        self.sum.add(value);
        if value <= 0.0 {
            self.zero_count.fetch_add(1, Ordering::Relaxed);
            return ZERO_BUCKET;
        }
        let index = index_of(value, self.schema);
        if index < self.min_index {
            self.counts[0].fetch_add(1, Ordering::Relaxed);
        } else {
            // Compute the slot in i64: only the *constructed* span is bounded,
            // so the distance from `min_index` to an extreme observation's
            // index can exceed i32 (e.g. a subnormal `min` and a ~1e300
            // value), and i32 arithmetic here would overflow. Bounds-check
            // before the cast; anything past the dense span is the open tail.
            let slot = i64::from(index) - i64::from(self.min_index);
            if slot < self.counts.len() as i64 {
                self.counts[slot as usize].fetch_add(1, Ordering::Relaxed);
            } else {
                self.overflow_count.fetch_add(1, Ordering::Relaxed);
            }
        }
        index
    }

    /// Records a duration observation, converting to seconds (`f64`).
    pub fn observe_duration(&self, value: Duration) -> i32 {
        self.observe(value.as_secs_f64())
    }

    /// The sum of all observed values so far, in the base unit.
    #[must_use]
    pub fn sum(&self) -> f64 {
        self.sum.get()
    }

    /// The total number of observations recorded so far.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.zero_count.load(Ordering::Relaxed)
            + self.overflow_count.load(Ordering::Relaxed)
            + self
                .counts
                .iter()
                .map(|c| c.load(Ordering::Relaxed))
                .sum::<u64>()
    }

    /// Takes a sparse, non-cumulative snapshot.
    #[must_use]
    pub fn snapshot(&self) -> ExponentialSnapshot {
        let populated = self
            .counts
            .iter()
            .enumerate()
            .filter_map(|(slot, counter)| {
                let count = counter.load(Ordering::Relaxed);
                (count > 0).then(|| (self.min_index + slot as i32, count, None))
            });
        ExponentialSnapshot::from_buckets(
            self.schema,
            self.zero_count.load(Ordering::Relaxed),
            self.overflow_count.load(Ordering::Relaxed),
            self.sum.get(),
            populated,
        )
    }

    /// The cumulative `le` view, for the classic OpenMetrics encoder.
    #[must_use]
    pub fn to_histogram_snapshot(&self) -> HistogramSnapshot {
        self.snapshot().to_histogram_snapshot()
    }
}

impl Default for FixedExponentialHistogram {
    fn default() -> Self {
        FixedExponentialHistogram::seconds_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_new_rejects_out_of_contract_requests_instead_of_clamping() {
        // [1e-9, 1e9] at schema 20 would need ~60 GB of buckets: rejected with
        // the requested count, not silently capped to a coarser resolution.
        assert!(matches!(
            FixedExponentialHistogram::try_new(1e-9, 1e9, 20).unwrap_err(),
            FixedHistogramError::TooManyBuckets { buckets } if buckets > MAX_FIXED_BUCKETS as u64
        ));
        // An out-of-range schema is reported, not clamped.
        assert_eq!(
            FixedExponentialHistogram::try_new(1.0, 10.0, MAX_SCHEMA + 1).unwrap_err(),
            FixedHistogramError::SchemaOutOfRange {
                schema: MAX_SCHEMA + 1
            }
        );
        assert!(matches!(
            FixedExponentialHistogram::try_new(0.0, 10.0, 3).unwrap_err(),
            FixedHistogramError::InvalidBounds { .. }
        ));
    }

    #[test]
    fn a_normal_range_constructs_and_counts() {
        let h = FixedExponentialHistogram::seconds_default();
        assert!(h.counts.len() < MAX_FIXED_BUCKETS);
        h.observe(0.01);
        assert_eq!(h.count(), 1);
    }

    #[test]
    fn an_extreme_observation_overflows_cleanly_not_the_slot_arithmetic() {
        // Only the constructed span is validated; an observed value can sit an
        // i32-overflowing index distance away from `min_index`. At schema 20 a
        // subnormal `min` puts `min_index` near -1.13e9 and 1e300 indexes near
        // +1.04e9: the i32 subtraction the observe path used to do overflows
        // (a debug-build panic). It must instead land in the open tail.
        let h = FixedExponentialHistogram::try_new(5e-324, 5e-324, 20).unwrap();
        h.observe(1e300);
        h.observe(f64::MAX);
        assert_eq!(h.count(), 2);
        let snap = h.snapshot();
        assert_eq!(snap.overflow_count, 2);
        assert!(snap.buckets.is_empty());

        // The tame direction still works: a value within the span is bucketed.
        h.observe(6e-324);
        assert_eq!(h.count(), 3);
        assert_eq!(h.snapshot().overflow_count, 2);
    }
}
