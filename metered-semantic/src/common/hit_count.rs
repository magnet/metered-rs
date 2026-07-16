//! A module providing the `HitCount` metric.

use crate::metric::{Measure, NoOpRecorder};
use metered::primitives::{Counter, CounterSource};
use std::ops::Deref;
use std::sync::atomic::AtomicU64;

/// A metric counting how many times an expression has been hit (entered).
///
/// This is a light-weight metric. The count is incremented on entry, so it is
/// recorded regardless of how the measured expression completes (including on
/// panic).
///
/// `HitCount` is a thin wrapper over a lock-free [`AtomicU64`]; reach the count
/// through its `Deref` (`hit_count.get()`).
#[derive(Default, Debug)]
pub struct HitCount(pub AtomicU64);

impl HitCount {
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

impl CounterSource for HitCount {
    fn get(&self) -> u64 {
        self.0.get()
    }
}

impl Counter for HitCount {
    fn incr_by(&self, n: u64) {
        self.0.incr_by(n);
    }
}

impl Measure for HitCount {
    type Recorder = NoOpRecorder;

    fn enter(&self) -> NoOpRecorder {
        self.incr();
        NoOpRecorder
    }
}

impl Deref for HitCount {
    type Target = AtomicU64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
