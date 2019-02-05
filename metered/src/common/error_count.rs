use crate::metric::{Counter, Metric};
use aspect::{Advice, Enter, OnResult};
use atomic::Atomic;

#[derive(Clone, Default, Debug)]
pub struct ErrorCount<C: Counter = Atomic<u64>>(C);

impl<C: Counter> Enter for ErrorCount<C> {
    type E = ();
    fn enter(&self) {}
}

impl<C: Counter, T, E> OnResult<Result<T, E>> for ErrorCount<C> {
    fn on_result(&self, _: (), r: &Result<T, E>) -> Advice {
        if r.is_err() {
            self.0.incr();
        };
        Advice::Return
    }
}
impl<C: Counter, T, E> Metric<Result<T, E>> for ErrorCount<C> {}
