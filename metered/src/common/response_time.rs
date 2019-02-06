//! A module providing the `ResponseTime` metric.

use crate::clear::Clear;
use crate::hdr_histogram::AtomicHdrHistogram;
use crate::metric::{Histogram, Metric};
use crate::time_source::{Instant, StdInstant};
use aspect::{Advice, Enter, OnResult};
use serde::Serialize;

/// A metric measuring the response time of an expression, that is the duration the expression needed to complete.
///
/// Because it retrieves the current time before calling the expression, computes the elapsed duration and registers it to an histogram, this is a rather heavy-weight metric better applied at entry-points.
///
/// By default, `ResponseTime` uses an atomic hdr histogram and a synchronized time source, which work better in multithread scenarios. Non-threaded applications can gain performance by using unsynchronized structures instead.
#[derive(Clone, Debug, Serialize)]
pub struct ResponseTime<H: Histogram = AtomicHdrHistogram, T: Instant = StdInstant>(
    H,
    std::marker::PhantomData<T>,
);

impl<H: Histogram, T: Instant> Default for ResponseTime<H, T> {
    fn default() -> Self {
        // A HdrHistogram measuring latencies from 1ms to 5minutes
        // All recordings will be saturating, that is, a value higher than 5 minutes
        // will be replace by 5 minutes...
        ResponseTime(H::with_bound(5 * 60 * 1000), std::marker::PhantomData)
    }
}

impl<H: Histogram, T: Instant, R> Metric<R> for ResponseTime<H, T> {}

impl<H: Histogram, T: Instant> Enter for ResponseTime<H, T> {
    type E = T;

    fn enter(&self) -> T {
        T::now()
    }
}

impl<H: Histogram, T: Instant, R> OnResult<R> for ResponseTime<H, T> {
    fn on_result(&self, enter: T, _: &R) -> Advice {
        let elapsed = enter.elapsed_millis();
        self.0.record(elapsed);
        Advice::Return
    }
}

impl<H: Histogram, T: Instant> Clear for ResponseTime<H, T> {
    fn clear(&self) {
        self.0.clear();
    }
}
