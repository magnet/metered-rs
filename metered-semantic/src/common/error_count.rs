//! A module providing the `ErrorCount` metric.

use crate::metric::{Armed, Measure, Recorder};
use metered::handle::Handle;
use metered::primitives::{Counter, CounterSource};
use std::ops::Deref;
use std::sync::atomic::AtomicU64;

/// A metric counting how many times an expression returning a std `Result`
/// returned an `Err` variant -- or was *aborted* by a panic, early exit, or
/// async cancellation, all of which count as errors.
///
/// This is a light-weight metric backed by a lock-free [`AtomicU64`].
#[derive(Default, Debug)]
pub struct ErrorCount(Handle<AtomicU64>);

impl ErrorCount {
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

impl CounterSource for ErrorCount {
    fn get(&self) -> u64 {
        self.0.get()
    }
}

impl Counter for ErrorCount {
    fn incr_by(&self, n: u64) {
        self.0.incr_by(n);
    }
}

impl Measure for ErrorCount {
    type Recorder = ErrorRecorder;

    fn enter(&self) -> ErrorRecorder {
        ErrorRecorder {
            counter: self.0.share(),
            armed: Armed::new(),
        }
    }
}

/// Recorder for [`ErrorCount`]. Increments the counter when the measured
/// expression returns `Err`, or when it is aborted (panic / cancellation).
#[derive(Debug)]
pub struct ErrorRecorder {
    counter: Handle<AtomicU64>,
    armed: Armed,
}

impl<T, E> Recorder<Result<T, E>> for ErrorRecorder {
    fn complete(&mut self, result: &Result<T, E>) {
        if self.armed.fire() && result.is_err() {
            self.counter.incr();
        }
    }
}

impl Drop for ErrorRecorder {
    fn drop(&mut self) {
        // Reached without `complete`: the expression panicked, returned early
        // through the guard, or its future was cancelled -- all errors.
        if self.armed.fire() {
            self.counter.incr();
        }
    }
}

impl Deref for ErrorCount {
    type Target = AtomicU64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
