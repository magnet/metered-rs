//! A module providing the `HitCount` metric.

use crate::{
    atomic::AtomicInt,
    clear::Clear,
    metric::{Counter, Metric},
};
use aspect::{Enter, OnResult};
use serde::Serialize;

/// A metric counting how many times an expression as been hit, before it
/// returns.
///
/// This is a light-weight metric.
///
/// By default, `HitCount` uses a lock-free `u64` `Counter`, which makes sense
/// in multithread scenarios. Non-threaded applications can gain performance by
/// using a `std::cell:Cell<u64>` instead.
#[derive(Clone, Default, Debug, Serialize)]
pub struct HitCount<C: Counter = AtomicInt<u64>>(pub C);

impl<C: Counter, R> Metric<R> for HitCount<C> {}

impl<C: Counter> Enter for HitCount<C> {
    type E = ();
    fn enter(&self) -> Self::E {
        self.0.incr();
    }
}

impl<C: Counter, R> OnResult<R> for HitCount<C> {}

impl<C: Counter> Clear for HitCount<C> {
    fn clear(&self) {
        self.0.clear()
    }
}
