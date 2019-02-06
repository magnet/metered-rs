use crate::atomic::AtomicInt;
use crate::clear::Clear;
use crate::metric::{Counter, Metric};
use aspect::{Enter, OnResult};
use serde::Serialize;

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
