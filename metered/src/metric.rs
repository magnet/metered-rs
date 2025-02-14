//! A module defining the [`Metric`] trait and common metric backends.

use crate::clear::{Clear, Clearable};
/// Re-export `aspect-rs`'s types to avoid crates depending on it.
pub use aspect::{Advice, Enter, OnResult, OnResultMut};
use serde::Serialize;
use std::marker::PhantomData;

/// A trait to implement to be used in the `measure!` macro
///
/// Metrics wrap expressions to measure them.
///
/// The return type, R, of the expression can be captured to perform special
/// handling.
pub trait Metric<R>: Default + OnResultMut<R> + Clear + Serialize {}

// Needed to force `measure!` to work only with the [`Metric`] trait.
#[doc(hidden)]
pub fn on_result<R, A: Metric<R>>(metric: &A, _enter: <A as Enter>::E, _result: &mut R) -> Advice {
    metric.on_result(_enter, _result)
}
/// Handles a metric's lifecycle, guarding against early returns and panics.
pub struct ExitGuard<'a, R, M: Metric<R>> {
    metric: &'a M,
    enter: Option<<M as Enter>::E>,
    _phantom: PhantomData<R>,
}

impl<'a, R, M: Metric<R>> ExitGuard<'a, R, M> {
    /// Enter a metric and create the guard for its exit.
    /// This calls [`aspect::Enter::enter`] on the metric internally.
    pub fn new(metric: &'a M) -> Self {
        Self {
            metric,
            enter: Some(metric.enter()),
            _phantom: PhantomData,
        }
    }

    /// If no unexpected exit occurred, record the expression's result.
    pub fn on_result(mut self, result: &mut R) {
        if let Some(enter) = self.enter.take() {
            self.metric.on_result(enter, result);
        } else {
            // OnResult called twice - we ignore
        }
    }
}

impl<R, M: Metric<R>> Drop for ExitGuard<'_, R, M> {
    fn drop(&mut self) {
        if let Some(enter) = self.enter.take() {
            self.metric.leave_scope(enter);
        } else {
            // on_result was called, so the result was already recorded
        }
    }
}

/// A trait for Counters
pub trait Counter: Default + Clear + Clearable + Serialize {
    /// Increment the counter
    fn incr(&self) {
        self.incr_by(1)
    }

    /// Increment the counter by count in one step
    ///
    /// Supplying a count larger than the underlying counter's remaining
    /// capacity will wrap like [`u8::wrapping_add`] and similar methods.
    fn incr_by(&self, count: usize);
}

/// A trait for Gauges
pub trait Gauge: Default + Clear + Serialize {
    /// Increment the counter
    fn incr(&self) {
        self.incr_by(1)
    }

    /// Decrement the counter
    fn decr(&self) {
        self.decr_by(1)
    }

    /// Increment the gauge by count in one step
    ///
    /// Supplying a count larger than the underlying counter's remaining
    /// capacity will wrap like [`u8::wrapping_add`] and similar methods.
    fn incr_by(&self, count: usize);

    /// Decrement the gauge by count in one step
    ///
    /// Supplying a count larger than the underlying counter's current value
    /// will wrap like [`u8::wrapping_sub`] and similar methods.
    fn decr_by(&self, count: usize);
}

/// A trait for Histograms
pub trait Histogram: Clear + Serialize {
    /// Build a new histogram with the given max bounds
    fn with_bound(max_value: u64) -> Self;

    /// Record a value to the histogram.
    ///
    /// It will saturate if the value is higher than the histogram's
    /// `max_value`.
    fn record(&self, value: u64);
}
