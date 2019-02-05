pub use aspect::{Advice, Enter, OnResult};

use serde::Serialize;

pub trait Metric<R>: Default + OnResult<R> +  Serialize {}

pub fn on_result<R, A: Metric<R>>(metric: &A, _enter: <A as Enter>::E, _result: &R) -> Advice {
    metric.on_result(_enter, _result)
}

pub trait Counter: Default + Serialize {
    fn incr(&self);
}

pub trait Gauge: Default + Serialize {
    fn incr(&self);

    fn decr(&self);
}

pub trait Histogram: Default + Serialize {
    fn record(&self, value: u64);
}
