//! A module defining the `Metric` trait and common metric backends.

use crate::clear::Clear;
/// Re-export `aspect-rs`'s types to avoid crates depending on it.
pub use aspect::{Advice, Enter, OnResult};
use serde::Serialize;

/// A trait to implement to be used in the `measure!` macro
///
/// Metrics wrap expressions to measure them.
///
/// The return type, R, of the expression can be captured to perform special handling.
pub trait Metric<R>: Default + OnResult<R> + Clear + Serialize {}

// Needed to force `measure!` to work only with the `Metric` trait.
#[doc(hidden)]
pub fn on_result<R, A: Metric<R>>(metric: &A, _enter: <A as Enter>::E, _result: &R) -> Advice {
    metric.on_result(_enter, _result)
}

/// A trait for Counters
pub trait Counter: Default + Clear + Serialize {
    /// Increment the counter
    fn incr(&self);
}

/// A trait for Gauges
pub trait Gauge: Default + Clear + Serialize {
    /// Increment the counter
    fn incr(&self);

    /// Decrement the counter
    fn decr(&self);
}

/// A trait for Histograms
pub trait Histogram: Clear + Serialize {
    /// Build a new histogram with the given max bounds
    fn with_bound(max_value: u64) -> Self;

    /// Record a value to the histogram.
    ///
    /// It will saturate if the value is higher than the histogram's `max_value`.
    fn record(&self, value: u64);
}
