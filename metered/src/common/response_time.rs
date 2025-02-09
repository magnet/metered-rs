//! A module providing the `ResponseTime` metric.

use crate::{
    clear::Clear,
    hdr_histogram::AtomicHdrHistogram,
    metric::{Histogram, Metric},
    time_source::{Instant, StdInstant},
};
use aspect::{Advice, Enter, OnResult};
use serde::{Serialize, Serializer};
use std::{ops::Deref, time::Duration};

/// A metric measuring the response time of an expression, that is the duration
/// the expression needed to complete.
///
/// Because it retrieves the current time before calling the expression,
/// computes the elapsed duration and registers it to an histogram, this is a
/// rather heavy-weight metric better applied at entry-points.
///
/// By default, `ResponseTime` uses an atomic hdr histogram and a synchronized
/// time source, which work better in multithread scenarios. Non-threaded
/// applications can gain performance by using unsynchronized structures
/// instead.
#[derive(Clone)]
pub struct ResponseTime<H: Histogram = AtomicHdrHistogram, T: Instant = StdInstant>(
    pub H,
    std::marker::PhantomData<T>,
);

impl<H: Histogram, T: Instant> ResponseTime<H, T> {
    /// Build a ResponseTime with a custom histogram bound
    ///
    /// ```rust
    /// use std::time::Duration;
    /// use metered::{ResponseTime, hdr_histogram::AtomicHdrHistogram, time_source::StdInstantMicros};
    ///
    /// let response_time_millis: ResponseTime =
    ///     ResponseTime::with_bound(Duration::from_secs(4));
    ///
    /// assert_eq!(response_time_millis.histogram().bound(), 4_000);
    ///
    /// let response_time_micros: ResponseTime<AtomicHdrHistogram, StdInstantMicros> =
    ///     ResponseTime::with_bound(Duration::from_secs(4));
    ///
    /// assert_eq!(response_time_micros.histogram().bound(), 4_000_000);
    /// ```
    pub fn with_bound(bound: Duration) -> Self {
        ResponseTime(H::with_bound(T::units(bound)), std::marker::PhantomData)
    }
}

impl<H: Histogram, T: Instant> Default for ResponseTime<H, T> {
    fn default() -> Self {
        // A HdrHistogram measuring latencies from 1ms to 5minutes
        // All recordings will be saturating, that is, a value higher than 5 minutes
        // will be replace by 5 minutes...
        ResponseTime(H::with_bound(5 * 60 * T::ONE_SEC), std::marker::PhantomData)
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
    fn leave_scope(&self, enter: T) -> Advice {
        let elapsed = enter.elapsed_time();
        self.0.record(elapsed);
        Advice::Return
    }
}

impl<H: Histogram, T: Instant> Clear for ResponseTime<H, T> {
    fn clear(&self) {
        self.0.clear();
    }
}

impl<H: Histogram + Serialize, T: Instant> Serialize for ResponseTime<H, T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(&self.0, serializer)
    }
}

use std::{fmt, fmt::Debug};
impl<H: Histogram + Debug, T: Instant> Debug for ResponseTime<H, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &self.0)
    }
}

impl<H: Histogram, T: Instant> Deref for ResponseTime<H, T> {
    type Target = H;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
