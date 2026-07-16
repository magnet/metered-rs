//! Adapting existing application state into native OpenMetrics output.
//!
//! Legacy semantic metrics (`HitCount`, `Elapsed`, ...) own
//! purely-observability state. But a value is often *already* owned by the
//! application -- an `AtomicBool` feature flag, a queue's length, a config
//! value -- and you simply want to expose it as a metric without keeping a
//! duplicate copy. metered's adapters close that gap: the `*Fn` adapters read
//! the value at encode time, so the exported metric is always the live value
//! the application uses.
//!
//! ```
//! use std::sync::atomic::{AtomicBool, Ordering};
//! use metered::adapter::flag;
//! use metered::MetricTree;
//! use metered_om::OpenMetricsEncoder;
//!
//! // An `enabled` flag the application already owns and uses:
//! let enabled = AtomicBool::new(true);
//!
//! // Expose it as a gauge (0/1) -- no separate metric to keep in sync.
//! let metric = flag(|| enabled.load(Ordering::Relaxed));
//!
//! let mut buf = String::new();
//! {
//!     let mut enc = OpenMetricsEncoder::new(&mut buf);
//!     metric.encode("feature_enabled", &[], &mut enc).unwrap();
//!     enc.finish().unwrap();
//! }
//! assert!(buf.contains("feature_enabled 1"));
//! ```
//!
//! For a custom leaf type, implement [`Metric`] so the
//! OpenMetrics type and sample encoding live in one place. For a custom tree,
//! implement [`MetricTree`](crate::MetricTree) directly and read your own fields
//! through the encoder helpers.

use crate::values::MetricValues;
use crate::{CounterSource, GaugeSource, Metric, MetricType};

/// Exposes a value, read at encode time, as an OpenMetrics gauge.
///
/// Wrap any `Fn() -> i64` that reads the live value:
/// `GaugeFn(|| queue.len() as i64)`.
///
/// An adapter is read-only by construction, so it implements only
/// [`GaugeSource`], never [`Gauge`](crate::Gauge): it cannot be handed to code
/// that expects to mutate the gauge, because the application owns the state.
pub struct GaugeFn<F>(pub F);

impl<F: Fn() -> i64> GaugeSource for GaugeFn<F> {
    type Value = i64;

    fn get(&self) -> Self::Value {
        (self.0)()
    }
}

impl<F: Fn() -> i64> Metric for GaugeFn<F> {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.gauge(name, labels, self.get());
    }
}

/// Exposes a value, read at encode time, as an OpenMetrics counter.
///
/// The closure must read a monotonically non-decreasing value (e.g. a running
/// total the application already maintains): `CounterFn(|| self.processed())`.
///
/// An adapter is read-only by construction, so it implements only
/// [`CounterSource`], never [`Counter`](crate::Counter): it cannot be handed to
/// code that expects to increment it, because the application owns the state.
pub struct CounterFn<F>(pub F);

impl<F: Fn() -> u64> CounterSource for CounterFn<F> {
    fn get(&self) -> u64 {
        (self.0)()
    }
}

impl<F: Fn() -> u64> Metric for CounterFn<F> {
    fn metric_type(&self) -> MetricType {
        MetricType::Counter
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, self.get());
    }
}

/// Exposes a closure as a counter.
pub fn counter<F: Fn() -> u64>(read: F) -> CounterFn<F> {
    CounterFn(read)
}

/// Exposes a closure as a gauge.
pub fn gauge<F: Fn() -> i64>(read: F) -> GaugeFn<F> {
    GaugeFn(read)
}

/// Exposes a boolean read at encode time as a gauge (`1` for `true`, `0` for
/// `false`) -- the idiomatic OpenMetrics representation of an on/off flag.
pub fn flag<F: Fn() -> bool>(read: F) -> GaugeFn<impl Fn() -> i64> {
    GaugeFn(move || read() as i64)
}

// The OpenMetrics rendering of these adapters is tested in
// `metered-om/tests/render_trees.rs` (the encoder lives in that
// crate).
