//! A module providing the `Throughput` metric.

use crate::{
    clear::Clear,
    metric::Metric,
    time_source::{Instant, StdInstant},
};
use aspect::{Advice, Enter, OnResult};
use serde::{Serialize, Serializer};
use std::ops::Deref;

mod atomic_tps;
mod tx_per_sec;

pub use atomic_tps::AtomicTxPerSec;
pub use tx_per_sec::TxPerSec;

/// A metric providing a transaction per second count backed by a histogram.
///
/// Because it retrieves the current time before calling the expression, stores
/// it to appropriately build time windows of 1 second and registers results to
/// a histogram, this is a rather heavy-weight metric better applied at
/// entry-points.
///
/// By default, `Throughput` uses an atomic transaction count backend and a
/// synchronized time source, which work better in multithread scenarios.
/// Non-threaded applications can gain performance by using unsynchronized
/// structures instead.
#[derive(Clone)]
pub struct Throughput<T: Instant = StdInstant, P: RecordThroughput = AtomicTxPerSec<T>>(
    pub P,
    std::marker::PhantomData<T>,
);

pub trait RecordThroughput: Default {
    fn on_result(&self);
}

impl<P: RecordThroughput, T: Instant> Default for Throughput<T, P> {
    fn default() -> Self {
        Throughput(P::default(), std::marker::PhantomData)
    }
}

impl<P: RecordThroughput + Serialize + Clear, T: Instant, R> Metric<R> for Throughput<T, P> {}

impl<P: RecordThroughput, T: Instant> Enter for Throughput<T, P> {
    type E = ();

    fn enter(&self) {}
}

impl<P: RecordThroughput + Clear, T: Instant> Clear for Throughput<T, P> {
    fn clear(&self) {
        self.0.clear();
    }
}

impl<P: RecordThroughput + Serialize, T: Instant, R> OnResult<R> for Throughput<T, P> {
    fn leave_scope(&self, _enter: ()) -> Advice {
        self.0.on_result();
        Advice::Return
    }
}

impl<P: RecordThroughput + Serialize, T: Instant> Serialize for Throughput<T, P> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(&self.0, serializer)
    }
}

use std::{fmt, fmt::Debug};
impl<P: RecordThroughput + Debug, T: Instant> Debug for Throughput<T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &self.0)
    }
}

impl<P: RecordThroughput, T: Instant> Deref for Throughput<T, P> {
    type Target = P;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
