pub use aspect::{Advice, Enter, OnResult};

pub trait Metric<R>: Default + OnResult<R> {}

pub fn on_result<R, A: Metric<R>>(metric: &A, _enter: <A as Enter>::E, _result: &R) -> Advice {
    metric.on_result(_enter, _result)
}

pub trait Counter: Default {
    fn incr(&self);
}

pub trait Gauge: Default {
    fn incr(&self);

    fn decr(&self);
}

pub trait Histogram: Default {
    fn record(&self, value: u64);
}
