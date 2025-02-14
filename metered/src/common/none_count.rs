//! A module providing the `NoneCount` metric.

use crate::{
    atomic::AtomicInt,
    clear::Clear,
    metric::{Counter, Metric},
};
use aspect::{Advice, Enter, OnResult};
use serde::Serialize;
use std::ops::Deref;

/// A metric counting how many times the return value is Ok(None) or None.
///
/// `SomeCount` is not provided since can be calculated by subtracting
/// `NoneCount` from `HitCount`.
///
/// This is a light-weight metric.
///
/// By default, `NoneCount` uses a lock-free `u64` `Counter`, which makes sense
/// in multithread scenarios. Non-threaded applications can gain performance by
/// using a `std::cell:Cell<u64>` instead.
#[derive(Clone, Default, Debug, Serialize)]
pub struct NoneCount<C: Counter = AtomicInt<u64>>(pub C);

impl<C: Counter, T, E> Metric<Result<Option<T>, E>> for NoneCount<C> {}

impl<C: Counter, T> Metric<Option<T>> for NoneCount<C> {}

impl<C: Counter> Enter for NoneCount<C> {
    type E = ();
    fn enter(&self) {}
}

impl<C: Counter, T, E> OnResult<Result<Option<T>, E>> for NoneCount<C> {
    fn on_result(&self, _: (), r: &Result<Option<T>, E>) -> Advice {
        if matches!(r, Ok(None)) {
            self.0.incr();
        }
        Advice::Return
    }
}

impl<C: Counter, T> OnResult<Option<T>> for NoneCount<C> {
    fn on_result(&self, _: (), r: &Option<T>) -> Advice {
        if r.is_none() {
            self.0.incr();
        }
        Advice::Return
    }
}

impl<C: Counter> Clear for NoneCount<C> {
    fn clear(&self) {
        self.0.clear();
    }
}

impl<C: Counter> Deref for NoneCount<C> {
    type Target = C;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
