use crate::hdr_histogram::AtomicHdrHistogram;
use crate::metric::{Histogram, Metric};
use aspect::{Advice, Enter, OnResult};
use serde::Serialize;

#[derive(Clone, Default, Debug, Serialize)]
pub struct ResponseTime<H: Histogram = AtomicHdrHistogram>(H);

impl<H: Histogram, R> Metric<R> for ResponseTime<H> {}

impl<H: Histogram> Enter for ResponseTime<H> {
    type E = std::time::Instant;

    fn enter(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

impl<H: Histogram, R> OnResult<R> for ResponseTime<H> {
    fn on_result(&self, enter: std::time::Instant, _: &R) -> Advice {
        let elapsed = enter.elapsed().as_millis() as u64;
        self.0.record(elapsed);
        Advice::Return
    }
}
