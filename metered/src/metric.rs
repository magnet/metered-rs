pub use aspect::{Advice, Enter, OnResult};
use crate::clear::Clear;
use serde::Serialize;

pub trait Metric<R>: Default + OnResult<R> + Clear + Serialize {}

pub fn on_result<R, A: Metric<R>>(metric: &A, _enter: <A as Enter>::E, _result: &R) -> Advice {
    metric.on_result(_enter, _result)
}

pub trait Counter: Default + Clear + Serialize {
    fn incr(&self);
}

pub trait Gauge: Default + Clear + Serialize {
    fn incr(&self);

    fn decr(&self);
}

pub trait Histogram: Clear + Serialize {
    fn with_bound(max_value: u64) -> Self;


    fn record(&self, value: u64);
}

