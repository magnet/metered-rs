#![feature(await_macro, async_await, futures_api)]

pub use metered_macro::metered;

pub trait Metric<R>: Default + Enter {
    fn on_result(&self, enter: <Self as Enter>::E, result: R) -> R {
        self.with_result(enter, &result);
        result
    }

    fn with_result(&self, _enter: <Self as Enter>::E, _result: &R) {}
}

pub trait Enter {
    type E;
    fn enter(&self) -> Self::E;
}

pub trait Counter: Default {
    type ValueType; // Something that looks like a number.

    fn incr(&self);
}

pub trait Gauge: Default {
    type ValueType; // Something that looks like a number.

    //fn get(&self) -> Self::ValueType ;
    fn incr(&self);

    fn decr(&self);
}

use atomic::Atomic;
use std::sync::atomic::Ordering;

#[macro_export]
macro_rules! impl_atomic_counter {
    ($int:path) => {
        impl Counter for Atomic<$int> {
            type ValueType = $int;

            fn incr(&self) {
                self.fetch_add(1, Ordering::Relaxed);
            }
        }
    };
}

impl_atomic_counter!(u8);
impl_atomic_counter!(u16);
impl_atomic_counter!(u32);
impl_atomic_counter!(u64);
impl_atomic_counter!(u128);
impl_atomic_counter!(usize);

#[macro_export]
macro_rules! impl_atomic_gauge {
    ($int:path) => {
        impl Gauge for Atomic<$int> {
            type ValueType = $int;

            fn incr(&self) {
                self.fetch_add(1, Ordering::Relaxed);
            }

            fn decr(&self) {
                self.fetch_sub(1, Ordering::Relaxed);
            }
        }
    };
}

impl_atomic_gauge!(u8);
impl_atomic_gauge!(u16);
impl_atomic_gauge!(u32);
impl_atomic_gauge!(u64);
impl_atomic_gauge!(u128);
impl_atomic_gauge!(usize);

#[derive(Clone, Default, Debug)]
pub struct HitCount<C: Counter = Atomic<u64>>(pub C);

impl<C: Counter> Enter for HitCount<C> {
    type E = ();
    fn enter(&self) -> Self::E {
        self.0.incr();
    }
}

impl<C: Counter, R> Metric<R> for HitCount<C> {}

#[derive(Clone, Default, Debug)]
pub struct ErrorCount<C: Counter = Atomic<u64>>(C);

impl<C: Counter> Enter for ErrorCount<C> {
    type E = ();
    fn enter(&self) {}
}

impl<C: Counter, T, E> Metric<Result<T, E>> for ErrorCount<C> {
    fn with_result(&self, _: (), r: &Result<T, E>) {
        if r.is_err() {
            self.0.incr();
        }
    }
}

#[derive(Clone, Default, Debug)]
pub struct InFlight<G: Gauge = Atomic<u64>>(G);

impl<G: Gauge> Enter for InFlight<G> {
    type E = ();
    fn enter(&self) {
        self.0.incr();
    }
}

impl<G: Gauge, R> Metric<R> for InFlight<G> {
    fn with_result(&self, _: (), _: &R) {
        self.0.decr();
    }
}

#[macro_export]
macro_rules! measure {
    ($metric:expr, $e:expr) => {
        $metric.on_result($metric.enter(), $e)
    };
}

#[derive(Clone, Default, Debug)]
pub struct ResponseTime<H: Histogram = AtomicHdrHistogram>(H);

impl<H: Histogram> Enter for ResponseTime<H> {
    type E = std::time::Instant;

    fn enter(&self) -> std::time::Instant {
        std::time::Instant::now()
    }
}

impl<H: Histogram, R> Metric<R> for ResponseTime<H> {
    fn with_result(&self, enter: std::time::Instant, _: &R) {
        let elapsed = enter.elapsed().as_millis() as u64;
        self.0.record(elapsed);
    }
}

pub trait Histogram: Default {
    fn record(&self, value: u64);
}

use atomic_refcell::AtomicRefCell;

#[derive(Default)]
pub struct AtomicHdrHistogram {
    inner: AtomicRefCell<HdrHistogram>,
}

impl Histogram for AtomicHdrHistogram {
    fn record(&self, value: u64) {
        self.inner.borrow_mut().record(value);
    }
}

use std::fmt;
use std::fmt::Debug;
impl Debug for AtomicHdrHistogram {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let histo = self.inner.borrow();
        write!(f, "AtomicHdrHistogram {{ {:?} }}", &histo)
    }
}

pub struct HdrHistogram {
    histo: hdrhistogram::Histogram<u64>,
}

impl HdrHistogram {
    fn record(&mut self, value: u64) {
        // All recordings will be saturating, that is, a value higher than 5 minutes
        // will be replace by 5 minutes...
        self.histo.saturating_record(value);
    }
}

impl Debug for HdrHistogram {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let hdr = &self.histo;
        let ile = |v| hdr.value_at_percentile(v);
        write!(
            f,
            "HdrHistogram {{ 
            samples: {}, min: {}, max: {}, mean: {}, stdev: {},
            90%ile = {}, 95%ile = {}, 99%ile = {}, 99.9%ile = {}, 99.99%ile = {} }}",
            hdr.len(),
            hdr.min(),
            hdr.max(),
            hdr.mean(),
            hdr.stdev(),
            ile(90.0),
            ile(95.0),
            ile(99.0),
            ile(99.9),
            ile(99.99)
        )
    }
}

impl Default for HdrHistogram {
    fn default() -> Self {
        // A HdrHistogram measuring latencies from 1ms to 5minutes
        // All recordings will be saturating, that is, a value higher than 5 minutes
        // will be replace by 5 minutes...
        let histo = hdrhistogram::Histogram::<u64>::new_with_bounds(1, 5 * 60 * 1000, 2)
            .expect("Could not instantiate HdrHistogram");

        HdrHistogram { histo }
    }
}

#[derive(Clone, Default)]
pub struct MetricRegistry {
    pub counters32: Vec<u32>,
}

impl MetricRegistry {
    pub fn new() -> MetricRegistry {
        Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_count_metrics() {
        let cc: HitCount = Default::default();
        // let mut cl: CallLatency<u64> = Default::default();

        let s = measure!(cc, { "hello world".to_string() });

        // let s = async_measure!(cc,  async { "foo" });

        println!("{}, {}", s, cc.0.load(Ordering::Relaxed));
    }

}
