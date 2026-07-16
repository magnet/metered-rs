//! Cumulative, Prometheus / OpenMetrics-style bucket histograms.
//!
//! Unlike the legacy HDR histogram module, this histogram does **not** compute
//! quantiles in-process and does **not** reset between scrapes. It records
//! observations into a fixed set of cumulative `le` buckets plus a running
//! `sum` and `count`, exactly the shape a Prometheus/VictoriaMetrics scraper
//! expects (`_bucket{le="..."}`, `_sum`, `_count`). Quantiles are computed at
//! query time with `histogram_quantile()`, which means they aggregate
//! correctly across replicas -- something pre-computed quantiles cannot do.
//!
//! Counting is lock-free: bucket counters are a fixed `Box<[AtomicU64]>`
//! allocated once, the total `count` is derived by summing them, and the sum is
//! an `AtomicU64` holding the bits of an `f64` updated with a compare-and-swap
//! loop. There is no allocation on the [`BucketHistogram::observe`] path and no
//! mutex. The optional per-bucket [`Exemplar`] store (see
//! [`BucketHistogram::observe_with_exemplar`]) is a fixed slice of per-bucket
//! [`ArcSwapOption`]s, so recording or reading an exemplar is lock-free too and
//! one bucket's exemplar never contends with another's.
//!
//! Values are recorded in the histogram's base unit. For durations that unit is
//! **seconds** (an `f64`), per Prometheus/OpenTelemetry convention -- never
//! milliseconds or microseconds. "Microsecond resolution" is achieved by
//! choosing finer bucket boundaries (see [`Buckets::fast_seconds`]), not by
//! changing the unit.

use crate::instruments::atomic_f64::AtomicF64;
use arc_swap::ArcSwapOption;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The bucket upper bounds (the `le` boundaries) of a [`BucketHistogram`],
/// expressed in the histogram's base unit (seconds for durations).
///
/// Bounds are finite, strictly ascending and de-duplicated. The implicit
/// `+Inf` bucket is always present and is **not** part of this list.
///
/// # Choosing buckets
///
/// Boundaries are a fixed, up-front cost: each one is an extra time series per
/// label combination at scrape time, so keep the count modest. 8–15 finite
/// bounds is typical and almost always enough; past ~20 you are usually paying
/// cardinality for resolution a dashboard can't use. Rules of thumb:
///
/// - **Latency, or anything spanning orders of magnitude:** use an exponential
///   layout ([`Buckets::exponential_range`], [`Buckets::relative`], or a preset
///   such as [`Buckets::seconds_default`] / [`Buckets::fast_seconds`]). Equal
///   *relative* spacing puts resolution where it matters and keeps the count
///   small.
/// - **Bounded, roughly uniform quantities** (payload sizes, queue depth, fill
///   ratios): [`Buckets::linear`] over the known `[low, high]`.
/// - **Put a boundary on values you alert on** (e.g. an SLO at 250ms) so
///   `histogram_quantile()` and threshold queries are accurate there.
/// - Boundaries only need to *bracket* the bulk of the distribution; anything
///   above the top bound lands in `+Inf` and is still counted.
#[derive(Clone, Debug, PartialEq)]
pub struct Buckets {
    /// Finite upper bounds, ascending. `+Inf` is implicit and not stored.
    bounds: Vec<f64>,
}

impl Buckets {
    /// Builds a bucket boundary set from an arbitrary iterator of finite upper
    /// bounds.
    ///
    /// Non-finite values (`NaN`, `±Inf`) and values `<= 0` are dropped; the
    /// remaining bounds are sorted ascending and de-duplicated. The implicit
    /// `+Inf` bucket is added by the histogram, so it must not be supplied
    /// here.
    ///
    /// ```
    /// use metered::bucket_histogram::Buckets;
    /// let b = Buckets::custom([0.01, 0.005, 0.01, 0.025]);
    /// assert_eq!(b.bounds(), &[0.005, 0.01, 0.025]);
    /// ```
    pub fn custom(bounds: impl IntoIterator<Item = f64>) -> Self {
        let mut bounds: Vec<f64> = bounds
            .into_iter()
            .filter(|b| b.is_finite() && *b > 0.0)
            .collect();
        bounds.sort_by(f64::total_cmp);
        bounds.dedup();
        Buckets { bounds }
    }

    /// The OpenTelemetry default duration buckets, in seconds.
    ///
    /// `[0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1, 2.5, 5,
    /// 7.5, 10]`. Sensible for most request/response latencies but too coarse
    /// for sub-5ms services -- see [`Buckets::fast_seconds`].
    pub fn seconds_default() -> Self {
        Buckets::custom([
            0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
        ])
    }

    /// Fine-grained duration buckets for fast (sub-5ms) services, in seconds,
    /// reaching down to microsecond resolution.
    ///
    /// `[25us, 50us, 100us, 250us, 500us, 1ms, 2.5ms, 5ms, 10ms, 25ms, 50ms,
    /// 100ms]`. Note the unit is still seconds (`0.000_025` == 25us); only the
    /// boundaries are finer.
    pub fn fast_seconds() -> Self {
        Buckets::custom([
            0.000_025, 0.000_05, 0.000_1, 0.000_25, 0.000_5, 0.001, 0.002_5, 0.005, 0.01, 0.025,
            0.05, 0.1,
        ])
    }

    /// Coarse duration buckets for slow services (DB-heavy work, batch jobs),
    /// in seconds, reaching out to a minute.
    ///
    /// `[10ms, 50ms, 100ms, 250ms, 500ms, 1s, 2.5s, 5s, 10s, 30s, 60s]`.
    pub fn slow_seconds() -> Self {
        Buckets::custom([0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0])
    }

    /// Wide-dynamic-range duration buckets, in seconds: fine near a microsecond,
    /// coarse near multi-second timeouts.
    ///
    /// Exponential boundaries from 1µs to ~30s at <=30% relative error
    /// ([`Buckets::relative`]) -- sub-microsecond resolution on a fast path,
    /// degrading to seconds near 10/20/30s timeouts (anything slower lands in the
    /// `+Inf` bucket). Use it for services or internal operations whose latency
    /// spans many orders of magnitude. It is more buckets (~70) than the other
    /// presets; tune the floor/ceiling/error with [`Buckets::relative`] if that
    /// cardinality is too high.
    pub fn wide_seconds() -> Self {
        Buckets::relative(0.000_001, 30.0, 0.3)
    }

    /// `count` **linearly**-spaced boundaries spanning `[low, high]` inclusive.
    ///
    /// Use this for quantities that are roughly uniform over a known range (a
    /// payload size, a queue depth, a fill ratio). For latency prefer an
    /// exponential layout ([`Buckets::exponential_range`] / [`Buckets::relative`]):
    /// latency spans orders of magnitude, and linear buckets waste resolution at
    /// the low end while staying too coarse to separate fast paths.
    ///
    /// `low` must be `> 0` (bounds `<= 0` are dropped by [`Buckets::custom`]);
    /// `count < 2`, a non-finite endpoint, or `high <= low` yields a single
    /// `low` bound.
    ///
    /// ```
    /// use metered::bucket_histogram::Buckets;
    /// assert_eq!(Buckets::linear(1.0, 5.0, 5).bounds(), &[1.0, 2.0, 3.0, 4.0, 5.0]);
    /// ```
    pub fn linear(low: f64, high: f64, count: usize) -> Self {
        if count < 2 || !low.is_finite() || !high.is_finite() || high <= low {
            return Buckets::custom([low]);
        }
        let step = (high - low) / (count as f64 - 1.0);
        Buckets::custom((0..count).map(|i| low + step * i as f64))
    }

    /// `count` exponentially-spaced boundaries: `start`, `start*factor`,
    /// `start*factor^2`, ... Gives wide dynamic range at bounded relative error
    /// (HDR-histogram-like resolution) while staying aggregatable classic `le`
    /// buckets.
    ///
    /// (True sparse "native" histograms require the Prometheus protobuf format;
    /// in the text exposition, exponential `le` buckets are the equivalent.)
    ///
    /// ```
    /// use metered::bucket_histogram::Buckets;
    /// assert_eq!(Buckets::exponential(1.0, 2.0, 5).bounds(), &[1.0, 2.0, 4.0, 8.0, 16.0]);
    /// ```
    pub fn exponential(start: f64, factor: f64, count: usize) -> Self {
        let mut bounds = Vec::with_capacity(count);
        let mut value = start;
        for _ in 0..count {
            bounds.push(value);
            value *= factor;
        }
        Buckets::custom(bounds)
    }

    /// `count` exponentially-spaced boundaries spanning `[min, max]` inclusive,
    /// solving the growth factor so the last bound is `max`.
    pub fn exponential_range(min: f64, max: f64, count: usize) -> Self {
        if count < 2 || min <= 0.0 || max <= min {
            return Buckets::custom(if min > 0.0 { vec![min] } else { vec![] });
        }
        let growth = (max / min).powf(1.0 / (count as f64 - 1.0));
        let mut bounds = Vec::with_capacity(count);
        let mut value = min;
        for _ in 0..count {
            bounds.push(value);
            value *= growth;
        }
        Buckets::custom(bounds)
    }

    /// Exponential boundaries sized to a target **relative** resolution, covering
    /// `[min, max]`.
    ///
    /// You say how coarse you'll tolerate (`max_relative_error`, e.g. `0.1` for
    /// 10%) and the range; the bucket *count* is solved for you. Because the
    /// spacing is relative, the absolute resolution is fine at the low end and
    /// coarse at the high end -- exactly what you want when you care about
    /// microsecond-scale fast paths but only need rough numbers near multi-second
    /// timeouts. A bucket near value `v` is about `v * max_relative_error` wide
    /// (e.g. at 30% error: ~0.3 µs near 1 µs, ~0.3 s near 1 s).
    ///
    /// ```
    /// use metered::bucket_histogram::Buckets;
    /// // <=10% relative error from 1ms to 10s.
    /// let b = Buckets::relative(0.001, 10.0, 0.1);
    /// assert!(*b.bounds().first().unwrap() <= 0.001);
    /// assert!(*b.bounds().last().unwrap() >= 10.0);
    /// // Each step grows by at most 1 + error.
    /// assert!(b.bounds().windows(2).all(|w| w[1] / w[0] <= 1.1 + 1e-9));
    /// ```
    pub fn relative(min: f64, max: f64, max_relative_error: f64) -> Self {
        if min <= 0.0 || max <= min || max_relative_error <= 0.0 {
            return Buckets::custom(if min > 0.0 { vec![min] } else { vec![] });
        }
        let factor = 1.0 + max_relative_error;
        // Multiplicative steps needed to reach `max`, rounded up so the last
        // bound covers it; `+ 1` for the starting bound.
        let count = ((max / min).ln() / factor.ln()).ceil() as usize + 1;
        Buckets::exponential(min, factor, count)
    }

    /// [`Buckets::custom`] with the bounds given as [`Duration`]s (converted to
    /// seconds, the histogram's base unit).
    ///
    /// ```
    /// use std::time::Duration;
    /// use metered::bucket_histogram::Buckets;
    /// let b = Buckets::custom_duration([
    ///     Duration::from_millis(5),
    ///     Duration::from_millis(10),
    ///     Duration::from_millis(25),
    /// ]);
    /// assert_eq!(b.bounds(), &[0.005, 0.01, 0.025]);
    /// ```
    pub fn custom_duration(bounds: impl IntoIterator<Item = Duration>) -> Self {
        Buckets::custom(bounds.into_iter().map(|d| d.as_secs_f64()))
    }

    /// [`Buckets::linear`] with the range given as [`Duration`]s (converted to
    /// seconds, the histogram's base unit).
    ///
    /// ```
    /// use std::time::Duration;
    /// use metered::bucket_histogram::Buckets;
    /// let b = Buckets::linear_duration(Duration::ZERO, Duration::from_millis(100), 6);
    /// // 0s is dropped (bounds must be > 0); 20/40/60/80/100ms remain.
    /// assert_eq!(b.bounds().len(), 5);
    /// ```
    pub fn linear_duration(low: Duration, high: Duration, count: usize) -> Self {
        Buckets::linear(low.as_secs_f64(), high.as_secs_f64(), count)
    }

    /// [`Buckets::exponential`] with the `start` bound given as a [`Duration`]
    /// (`factor` and `count` are unitless).
    ///
    /// ```
    /// use std::time::Duration;
    /// use metered::bucket_histogram::Buckets;
    /// let b = Buckets::exponential_duration(Duration::from_millis(1), 2.0, 4);
    /// assert_eq!(b.bounds(), &[0.001, 0.002, 0.004, 0.008]);
    /// ```
    pub fn exponential_duration(start: Duration, factor: f64, count: usize) -> Self {
        Buckets::exponential(start.as_secs_f64(), factor, count)
    }

    /// [`Buckets::exponential_range`] with the range given as [`Duration`]s.
    ///
    /// The usual choice for latency: log-spaced boundaries across `[min, max]`
    /// so a microsecond fast path and a multi-second slow path both get useful
    /// resolution from a modest bucket count.
    ///
    /// ```
    /// use std::time::Duration;
    /// use metered::bucket_histogram::Buckets;
    /// let b = Buckets::exponential_range_duration(
    ///     Duration::from_micros(100),
    ///     Duration::from_secs(10),
    ///     12,
    /// );
    /// assert_eq!(b.bounds().len(), 12);
    /// assert!(*b.bounds().first().unwrap() <= 0.000_1);
    /// assert!(*b.bounds().last().unwrap() >= 10.0);
    /// ```
    pub fn exponential_range_duration(min: Duration, max: Duration, count: usize) -> Self {
        Buckets::exponential_range(min.as_secs_f64(), max.as_secs_f64(), count)
    }

    /// [`Buckets::relative`] with the range given as [`Duration`]s; the bucket
    /// count is solved from `max_relative_error` (e.g. `0.1` for <=10%).
    pub fn relative_duration(min: Duration, max: Duration, max_relative_error: f64) -> Self {
        Buckets::relative(min.as_secs_f64(), max.as_secs_f64(), max_relative_error)
    }

    /// The finite upper bounds, ascending. The implicit `+Inf` bucket is not
    /// included.
    pub fn bounds(&self) -> &[f64] {
        &self.bounds
    }

    /// The number of buckets the histogram will expose, including the implicit
    /// `+Inf` bucket.
    pub fn len(&self) -> usize {
        self.bounds.len() + 1
    }

    /// Returns `true` if there are no finite bounds (the histogram would be a
    /// single `+Inf` bucket). Such a configuration is almost never useful.
    pub fn is_empty(&self) -> bool {
        self.bounds.is_empty()
    }
}

impl Default for Buckets {
    fn default() -> Self {
        Buckets::seconds_default()
    }
}

/// An OpenMetrics exemplar: a set of labels (typically `trace_id` / `span_id`),
/// the observed value, and an optional unix timestamp in seconds.
///
/// metered stores exemplars but is deliberately agnostic about where they come
/// from -- it has no tracing dependency. A consumer (e.g. a service framework)
/// is responsible for minting exemplars from the active trace and for any
/// tail-sampling "keep" decision; [`BucketHistogram::observe`] returns the
/// landing bucket index precisely so that decision can be made cheaply.
#[derive(Clone, Debug, PartialEq)]
pub struct Exemplar {
    /// Exemplar labels, e.g. `[("trace_id", "..."), ("span_id", "...")]`.
    pub labels: Vec<(String, String)>,
    /// The observed value the exemplar refers to.
    pub value: f64,
    /// Optional unix timestamp, in seconds.
    pub timestamp_seconds: Option<f64>,
}

/// Supplies an [`Exemplar`] to attach to a measured observation.
///
/// metered is tracing-agnostic: a consumer implements this to mint an exemplar
/// from the active trace context (e.g. the current span's trace/span id) and to
/// drive any tail-sampling "keep" decision. Sources are expected to be cheap,
/// stateless types (often a unit struct) that read ambient context; the
/// observed value is filled in by the caller, so an implementation only needs
/// to provide the labels (and optional timestamp).
pub trait ExemplarSource: Clone {
    /// Returns an exemplar for the observation just recorded, or `None`.
    fn exemplar(&self) -> Option<Exemplar>;
}

/// The default [`ExemplarSource`]: never produces an exemplar.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoExemplars;

impl ExemplarSource for NoExemplars {
    fn exemplar(&self) -> Option<Exemplar> {
        None
    }
}

/// An ambient, thread-local exemplar context (behind the `exemplar-context`
/// feature).
///
/// metered takes no tracing/OpenTelemetry dependency. This is the dependency-free
/// seam for wiring exemplars to the active trace: a tracing layer sets the
/// current exemplar (the span's `trace_id` / `span_id`) for the duration of a
/// span, and any `Elapsed<ThreadLocalExemplars>` picks it up automatically. It is
/// opt-in because, unlike the rest of the crate, it relies on ambient state.
#[cfg(feature = "exemplar-context")]
pub use self::context::{set_current_exemplar, with_exemplar, ThreadLocalExemplars};

#[cfg(feature = "exemplar-context")]
mod context {
    use super::{Exemplar, ExemplarSource};
    use std::cell::RefCell;

    thread_local! {
        static CURRENT: RefCell<Option<Exemplar>> = const { RefCell::new(None) };
    }

    /// Sets (or clears) the current thread's exemplar. A tracing layer typically
    /// calls this on span enter/exit.
    pub fn set_current_exemplar(exemplar: Option<Exemplar>) {
        CURRENT.with(|current| *current.borrow_mut() = exemplar);
    }

    /// Runs `f` with `exemplar` set as the current exemplar, restoring the
    /// previous value afterwards (even on panic).
    pub fn with_exemplar<R>(exemplar: Exemplar, f: impl FnOnce() -> R) -> R {
        struct Restore(Option<Exemplar>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|current| *current.borrow_mut() = self.0.take());
            }
        }

        let previous = CURRENT.with(|current| current.borrow_mut().replace(exemplar));
        let _restore = Restore(previous);
        f()
    }

    /// An [`ExemplarSource`] that reads the ambient [`set_current_exemplar`]
    /// value, so `Elapsed<ThreadLocalExemplars>` attaches the active trace's
    /// exemplar to every observation.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct ThreadLocalExemplars;

    impl ExemplarSource for ThreadLocalExemplars {
        fn exemplar(&self) -> Option<Exemplar> {
            CURRENT.with(|current| current.borrow().clone())
        }
    }
}

/// A single cumulative bucket of a [`HistogramSnapshot`].
#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    /// The inclusive upper bound (`le`) of this bucket. The final bucket uses
    /// `f64::INFINITY` to represent `+Inf`.
    pub le: f64,
    /// The **cumulative** number of observations less than or equal to `le`.
    pub cumulative_count: u64,
    /// The exemplar last recorded into this bucket, if any.
    pub exemplar: Option<Exemplar>,
}

/// An immutable, point-in-time view of a [`BucketHistogram`], suitable for
/// encoding. Bucket counts are cumulative, matching the Prometheus exposition
/// format.
#[derive(Clone, Debug, PartialEq)]
pub struct HistogramSnapshot {
    /// Cumulative buckets, ascending by `le`, ending with the `+Inf` bucket
    /// whose `cumulative_count` always equals [`HistogramSnapshot::count`].
    pub buckets: Vec<Bucket>,
    /// The sum of all observed values, in the histogram's base unit.
    pub sum: f64,
    /// The total number of observations.
    pub count: u64,
}

/// A cumulative bucket histogram.
///
/// See the [module documentation](crate::bucket_histogram) for the rationale.
/// Construct one from a [`Buckets`] boundary set and record observations with
/// [`BucketHistogram::observe`] (raw base-unit value),
/// [`BucketHistogram::observe_duration`], or
/// [`BucketHistogram::observe_with_exemplar`].
#[derive(Debug)]
pub struct BucketHistogram {
    /// Finite upper bounds, ascending. Length `N`.
    bounds: Vec<f64>,
    /// Per-bucket counters. Length `N + 1`; the last entry is the `+Inf`
    /// bucket. These are **not** cumulative; [`BucketHistogram::snapshot`]
    /// accumulates them.
    counts: Box<[AtomicU64]>,
    /// Sum of observed values.
    sum: AtomicF64,
    /// One lock-free exemplar slot per bucket (length `N + 1`). Written only
    /// when an exemplar is supplied and read at snapshot time; each slot swaps
    /// independently, so no bucket contends with another and neither path
    /// locks.
    exemplars: Box<[ArcSwapOption<Exemplar>]>,
}

impl BucketHistogram {
    /// Builds a histogram over the given bucket boundaries.
    pub fn new(buckets: Buckets) -> Self {
        let bounds = buckets.bounds;
        let n = bounds.len() + 1;
        let counts = (0..n)
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let exemplars = (0..n)
            .map(|_| ArcSwapOption::empty())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        BucketHistogram {
            bounds,
            counts,
            sum: AtomicF64::new(0.0),
            exemplars,
        }
    }

    fn bucket_index(&self, value: f64) -> usize {
        // First bucket whose upper bound is >= value (i.e. `le` semantics).
        // `partition_point` counts the bounds strictly less than `value`, which
        // is exactly that bucket's index; if `value` exceeds every bound the
        // index is `bounds.len()`, the `+Inf` bucket.
        self.bounds.partition_point(|&b| b < value)
    }

    /// Records a single observation, in the histogram's base unit (seconds for
    /// durations).
    ///
    /// Returns the index of the bucket the value landed in (`0..len`), where
    /// the last index is the `+Inf` bucket. The index is returned so that an
    /// exemplar/tail-sampling layer can decide, cheaply and without a second
    /// lookup, whether the observation belongs to an outlier bucket.
    ///
    /// A `NaN` observation is dropped (no count, no sum, so a stray NaN never
    /// poisons `_sum`) and reported as bucket `0`.
    pub fn observe(&self, value: f64) -> usize {
        // Drop NaN before it can touch `sum` or a bucket count: a single NaN
        // would otherwise poison `_sum` forever.
        if value.is_nan() {
            return 0;
        }
        let idx = self.bucket_index(value);
        self.counts[idx].fetch_add(1, Ordering::Relaxed);
        self.sum.add(value);
        idx
    }

    /// Records an observation and attaches an [`Exemplar`] to the bucket the
    /// value lands in (replacing any previous exemplar for that bucket, per the
    /// OpenMetrics one-exemplar-per-bucket rule). Returns the bucket index.
    ///
    /// The counting is identical to [`BucketHistogram::observe`]; the exemplar
    /// is published into the bucket's slot with one lock-free swap. A `NaN`
    /// observation is dropped and its `exemplar` discarded.
    pub fn observe_with_exemplar(&self, value: f64, exemplar: Exemplar) -> usize {
        let idx = self.observe(value);
        if !value.is_nan() {
            self.exemplars[idx].store(Some(Arc::new(exemplar)));
        }
        idx
    }

    /// Records a duration observation, converting to seconds (`f64`) without
    /// quantising to milliseconds first, so sub-millisecond detail is retained.
    pub fn observe_duration(&self, value: Duration) -> usize {
        self.observe(value.as_secs_f64())
    }

    /// The total number of observations recorded so far.
    ///
    /// Derived by summing the (non-cumulative) per-bucket counters, so it is
    /// equal to the `+Inf` bucket's cumulative count by construction. There is
    /// no separate total atomic to keep in step on the write path.
    pub fn count(&self) -> u64 {
        self.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum()
    }

    /// The sum of all observed values so far, in the histogram's base unit.
    pub fn sum(&self) -> f64 {
        self.sum.get()
    }

    /// Takes a cumulative snapshot for encoding.
    ///
    /// Reads each atomic bucket once and accumulates, so the per-bucket counts
    /// in the result are cumulative (`le`-style) and `count` (the final
    /// cumulative) matches the `+Inf` bucket exactly. The snapshot is consistent
    /// enough for monitoring: under concurrent observation `sum` and the buckets
    /// may reflect slightly different instants, which is the same trade-off
    /// every lock-free Prometheus client makes.
    pub fn snapshot(&self) -> HistogramSnapshot {
        let mut cumulative = 0u64;
        let mut buckets = Vec::with_capacity(self.counts.len());
        for (idx, counter) in self.counts.iter().enumerate() {
            cumulative += counter.load(Ordering::Relaxed);
            let le = self.bounds.get(idx).copied().unwrap_or(f64::INFINITY);
            buckets.push(Bucket {
                le,
                cumulative_count: cumulative,
                exemplar: self.exemplars[idx].load_full().map(|arc| (*arc).clone()),
            });
        }
        HistogramSnapshot {
            buckets,
            sum: self.sum(),
            // The running cumulative after the final (`+Inf`) bucket is the
            // total, so `count` equals that bucket's `cumulative_count` from a
            // single pass over the same atomic reads.
            count: cumulative,
        }
    }
}

impl Default for BucketHistogram {
    fn default() -> Self {
        BucketHistogram::new(Buckets::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_sorts_dedups_and_drops_invalid() {
        let b = Buckets::custom([0.01, 0.005, 0.01, -1.0, f64::NAN, f64::INFINITY, 0.025]);
        assert_eq!(b.bounds(), &[0.005, 0.01, 0.025]);
        assert_eq!(b.len(), 4); // 3 finite + Inf
    }

    #[test]
    fn exponential_buckets_have_wide_dynamic_range() {
        assert_eq!(
            Buckets::exponential(1.0, 2.0, 5).bounds(),
            &[1.0, 2.0, 4.0, 8.0, 16.0]
        );
        let ranged = Buckets::exponential_range(0.001, 10.0, 8);
        let bounds = ranged.bounds();
        assert_eq!(bounds.len(), 8);
        assert!((bounds[0] - 0.001).abs() < 1e-9);
        assert!((bounds[bounds.len() - 1] - 10.0).abs() < 1e-6);
        // strictly increasing
        assert!(bounds.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn relative_buckets_bound_the_step_and_cover_the_range() {
        let b = Buckets::relative(0.000_001, 30.0, 0.3);
        let bounds = b.bounds();
        // Covers the whole range.
        assert!(*bounds.first().unwrap() <= 0.000_001 + 1e-12);
        assert!(*bounds.last().unwrap() >= 30.0);
        // Every step is within the relative error.
        assert!(bounds.windows(2).all(|w| w[1] / w[0] <= 1.3 + 1e-9));
        // Fine at the low end (sub-microsecond), coarse at the high end.
        assert!(bounds[1] - bounds[0] < 0.000_001); // < 1µs near the floor
                                                    // `wide_seconds` is exactly this recipe.
        assert_eq!(Buckets::wide_seconds().bounds(), bounds);
    }

    #[test]
    fn one_millisecond_lands_in_fast_one_milli_bucket() {
        // The RFC's conformance case: a known 1ms op must land in the right
        // sub-5ms bucket when using the fast preset (the OTel default would
        // dump it into le=0.005).
        let h = BucketHistogram::new(Buckets::fast_seconds());
        let idx = h.observe(0.001);
        let bounds = Buckets::fast_seconds();
        assert_eq!(bounds.bounds()[idx], 0.001, "1ms should land in le=0.001");
    }

    #[test]
    fn fifty_microseconds_keeps_resolution_in_seconds() {
        let h = BucketHistogram::new(Buckets::fast_seconds());
        let idx = h.observe(0.000_05); // 50us, expressed in seconds
        assert_eq!(Buckets::fast_seconds().bounds()[idx], 0.000_05);
        // Sum is in seconds and retains microsecond precision exactly.
        assert_eq!(h.sum(), 0.000_05);
    }

    #[test]
    fn value_above_all_bounds_lands_in_inf_bucket() {
        let h = BucketHistogram::new(Buckets::seconds_default());
        let idx = h.observe(42.0);
        assert_eq!(idx, Buckets::seconds_default().bounds().len()); // +Inf index
        let snap = h.snapshot();
        assert_eq!(snap.buckets.last().unwrap().le, f64::INFINITY);
        assert_eq!(snap.buckets.last().unwrap().cumulative_count, 1);
    }

    #[test]
    fn snapshot_is_cumulative_and_monotonic() {
        let h = BucketHistogram::new(Buckets::custom([1.0, 2.0, 3.0]));
        for v in [0.5, 1.5, 1.5, 2.5, 100.0] {
            h.observe(v);
        }
        let snap = h.snapshot();
        // le=1 :1, le=2 :3, le=3 :4, +Inf :5
        let counts: Vec<u64> = snap.buckets.iter().map(|b| b.cumulative_count).collect();
        assert_eq!(counts, vec![1, 3, 4, 5]);
        assert!(counts.windows(2).all(|w| w[0] <= w[1]), "must be monotonic");
        assert_eq!(snap.count, 5);
        assert_eq!(snap.sum, 0.5 + 1.5 + 1.5 + 2.5 + 100.0);
    }

    #[test]
    fn nan_observation_is_dropped_and_does_not_poison_sum() {
        let h = BucketHistogram::new(Buckets::custom([1.0, 2.0]));
        h.observe(1.5);
        h.observe(f64::NAN);
        h.observe(1.5);

        // The NaN never reaches `sum` or a bucket count: `sum` stays finite and
        // correct, and only the two real observations are counted.
        assert!(h.sum().is_finite());
        assert_eq!(h.sum(), 3.0);
        assert_eq!(h.count(), 2);
        let snap = h.snapshot();
        assert!(snap.sum.is_finite());
        assert_eq!(snap.count, 2);
    }

    #[test]
    fn concurrent_observes_are_lock_free_and_consistent() {
        use std::sync::Arc;
        use std::thread;

        let h = Arc::new(BucketHistogram::new(Buckets::fast_seconds()));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let h = Arc::clone(&h);
                thread::spawn(move || {
                    for _ in 0..10_000 {
                        h.observe(0.000_3);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(h.count(), 80_000);
        let snap = h.snapshot();
        assert_eq!(snap.buckets.last().unwrap().cumulative_count, 80_000);
    }

    #[test]
    fn exemplar_is_stored_in_its_bucket() {
        let h = BucketHistogram::new(Buckets::custom([1.0, 2.0]));
        let idx = h.observe_with_exemplar(
            1.5,
            Exemplar {
                labels: vec![("trace_id".into(), "abc123".into())],
                value: 1.5,
                timestamp_seconds: None,
            },
        );
        let snap = h.snapshot();
        assert_eq!(idx, 1); // le=2 bucket
        assert_eq!(
            snap.buckets[1].exemplar.as_ref().unwrap().labels[0].1,
            "abc123"
        );
        assert!(snap.buckets[0].exemplar.is_none());
    }

    #[cfg(feature = "exemplar-context")]
    #[test]
    fn thread_local_exemplar_source_reads_ambient_context() {
        use super::{with_exemplar, ExemplarSource, ThreadLocalExemplars};

        let source = ThreadLocalExemplars;
        assert!(source.exemplar().is_none());

        let exemplar = Exemplar {
            labels: vec![("trace_id".into(), "tid".into())],
            value: 0.0,
            timestamp_seconds: None,
        };
        with_exemplar(exemplar, || {
            assert_eq!(source.exemplar().unwrap().labels[0].1, "tid");
        });

        // Restored after the scope.
        assert!(source.exemplar().is_none());
    }
}
