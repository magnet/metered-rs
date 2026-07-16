//! A module providing the `NoneCount` metric.

use crate::metric::{Measure, Recorder};
use metered::handle::Handle;
use metered::primitives::{Counter, CounterSource};
use std::ops::Deref;
use std::sync::atomic::AtomicU64;

/// A metric counting how many times the return value is `Ok(None)` or `None`.
///
/// `SomeCount` is not provided since it can be calculated by subtracting
/// `NoneCount` from `HitCount`.
///
/// A panic / cancellation is *not* counted as a `None` (it has no value), so
/// this metric records only on normal completion.
///
/// This is a light-weight metric backed by a lock-free [`AtomicU64`].
#[derive(Default, Debug)]
pub struct NoneCount(Handle<AtomicU64>);

impl NoneCount {
    /// Increments the count by one.
    pub fn incr(&self) {
        Counter::incr(self);
    }

    /// Increments the count by `n`.
    pub fn incr_by(&self, n: u64) {
        Counter::incr_by(self, n);
    }

    /// Returns the current count.
    pub fn get(&self) -> u64 {
        CounterSource::get(self)
    }
}

impl CounterSource for NoneCount {
    fn get(&self) -> u64 {
        self.0.get()
    }
}

impl Counter for NoneCount {
    fn incr_by(&self, n: u64) {
        self.0.incr_by(n);
    }
}

impl Measure for NoneCount {
    type Recorder = NoneRecorder;

    fn enter(&self) -> NoneRecorder {
        NoneRecorder {
            counter: self.0.share(),
        }
    }
}

/// Recorder for [`NoneCount`]: increments when the completed result is "none".
#[derive(Debug)]
pub struct NoneRecorder {
    counter: Handle<AtomicU64>,
}

impl<T, E> Recorder<Result<Option<T>, E>> for NoneRecorder {
    fn complete(&mut self, result: &Result<Option<T>, E>) {
        if matches!(result, Ok(None)) {
            self.counter.incr();
        }
    }
}

impl<T> Recorder<Option<T>> for NoneRecorder {
    fn complete(&mut self, result: &Option<T>) {
        if result.is_none() {
            self.counter.incr();
        }
    }
}

impl Deref for NoneCount {
    type Target = AtomicU64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
