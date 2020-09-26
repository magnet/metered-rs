//! A module providing the `ErrorCount` metric.

use crate::{
    atomic::AtomicInt,
    clear::Clear,
    metric::{Counter, Metric},
};
use aspect::{Advice, Enter, OnResult};
use serde::Serialize;

/// A metric counting how many times an expression typed std `Result` as
/// returned an `Err` variant.
///
/// This is a light-weight metric.
///
/// By default, `ErrorCount` uses a lock-free `u64` `Counter`, which makes sense
/// in multithread scenarios. Non-threaded applications can gain performance by
/// using a `std::cell:Cell<u64>` instead.
#[derive(Clone, Default, Debug, Serialize)]
pub struct ErrorCount<C: Counter = AtomicInt<u64>>(pub C);

impl<C: Counter, T, E> Metric<Result<T, E>> for ErrorCount<C> {}

impl<C: Counter> Enter for ErrorCount<C> {
    type E = ();
    fn enter(&self) {}
}

impl<C: Counter, T, E> OnResult<Result<T, E>> for ErrorCount<C> {
    fn on_result(&self, _: (), r: &Result<T, E>) -> Advice {
        if r.is_err() {
            self.0.incr();
        }
        Advice::Return
    }
}

impl<C: Counter> Clear for ErrorCount<C> {
    fn clear(&self) {
        self.0.clear()
    }
}
