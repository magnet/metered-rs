use crate::atomic::AtomicInt;
use crate::clear::Clear;
use crate::metric::{Gauge, Metric};
use aspect::{Advice, Enter, OnResult};
use serde::Serialize;

#[derive(Clone, Default, Debug, Serialize)]
pub struct InFlight<G: Gauge = AtomicInt<u64>>(G);

impl<G: Gauge, R: Serialize> Metric<R> for InFlight<G> {}

impl<G: Gauge> Enter for InFlight<G> {
    type E = ();
    fn enter(&self) {
        self.0.incr();
    }
}

impl<G: Gauge, R> OnResult<R> for InFlight<G> {
    fn on_result(&self, _: (), _: &R) -> Advice {
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
