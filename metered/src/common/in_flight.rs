//! A module providing the `InFlight` metric.

use crate::{
    atomic::AtomicInt,
    clear::Clear,
    metric::{Gauge, Metric},
};
use aspect::{Advice, Enter, OnResult};
use serde::Serialize;
use std::ops::Deref;

/// A metric providing an in-flight gauge, showing how many calls are currently
/// active for an expression.
///
/// This is a light-weight metric.
///
/// This makes sense mostly in a multi-threaded situation where several threads
/// may call the same method constantly, and we want to monitor how many are
/// active at a given time.
///
/// The [`Throughput`] metric shows an alternative view
/// of the same picture, by reporting how many transactions per seconds are
/// processed by an expression.
///
/// By default, `InFlight` uses a lock-free `u64` [`Gauge`], which makes sense
/// in multithread scenarios. Non-threaded applications can gain performance by
/// using a `std::cell:Cell<u64>` instead.

#[derive(Clone, Default, Debug, Serialize)]
pub struct InFlight<G: Gauge = AtomicInt<u64>>(pub G);

impl<G: Gauge, R> Metric<R> for InFlight<G> {}

impl<G: Gauge> Enter for InFlight<G> {
    type E = ();
    fn enter(&self) {
        self.0.incr();
    }
}

impl<G: Gauge, R> OnResult<R> for InFlight<G> {
    fn leave_scope(&self, _: ()) -> Advice {
        self.0.decr();
        Advice::Return
    }
}

impl<G: Gauge> Clear for InFlight<G> {
    fn clear(&self) {
        // Do nothing: an InFlight metric
        // would get in an inconsistent state if cleared
    }
}

impl<G: Gauge> Deref for InFlight<G> {
    type Target = G;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
