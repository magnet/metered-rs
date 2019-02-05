use crate::metric::{Counter, Metric};
use aspect::{Enter, OnResult};
use atomic::Atomic;

#[derive(Clone, Default, Debug)]
pub struct HitCount<C: Counter = Atomic<u64>>(pub C);
impl<C: Counter, R> Metric<R> for HitCount<C> {}

impl<C: Counter> Enter for HitCount<C> {
    type E = ();
    fn enter(&self) -> Self::E {
        self.0.incr();
    }
}

impl<C: Counter, R> OnResult<R> for HitCount<C> {}
