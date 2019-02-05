use crate::metric::{Gauge, Metric};
use aspect::{Advice, Enter, OnResult};
use atomic::Atomic;

#[derive(Clone, Default, Debug)]
pub struct InFlight<G: Gauge = Atomic<u64>>(G);

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
impl<G: Gauge, R> Metric<R> for InFlight<G> {}
