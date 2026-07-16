//! A module providing the `InFlight` metric.

use crate::metric::{Armed, Measure, Recorder};
use metered::handle::Handle;
use metered::primitives::{Gauge, GaugeSource};
use std::ops::Deref;
use std::sync::atomic::AtomicI64;

/// A metric providing an in-flight gauge, showing how many calls are currently
/// active for an expression.
///
/// This is a light-weight metric, mostly useful in multi-threaded situations
/// where we want to monitor how many calls are active at a given time.
///
/// The gauge is incremented on entry and decremented when the recorder is
/// finished -- on normal completion *or* on panic / early-exit / async
/// cancellation -- so it can never leak. Because the recorder owns a shared
/// handle to the gauge rather than borrowing it, the measured expression is
/// free to take `&mut self` and to `.await`.
///
/// `InFlight` is a thin wrapper over a lock-free [`AtomicI64`].
#[derive(Default, Debug)]
pub struct InFlight(Handle<AtomicI64>);

impl InFlight {
    /// Sets the current value.
    pub fn set(&self, value: i64) {
        Gauge::set(self, value);
    }

    /// Adds `delta` to the current value.
    pub fn add(&self, delta: i64) {
        Gauge::add(self, delta);
    }

    /// Increments the gauge by one.
    pub fn incr(&self) {
        Gauge::incr(self);
    }

    /// Decrements the gauge by one.
    pub fn decr(&self) {
        Gauge::decr(self);
    }

    /// Returns the current value.
    pub fn get(&self) -> i64 {
        GaugeSource::get(self)
    }
}

impl GaugeSource for InFlight {
    type Value = i64;

    fn get(&self) -> Self::Value {
        self.0.get()
    }
}

impl Gauge for InFlight {
    fn set(&self, value: Self::Value) {
        self.0.set(value);
    }

    fn add(&self, delta: Self::Value) {
        self.0.add(delta);
    }

    fn incr(&self) {
        self.0.incr();
    }

    fn try_decr(&self) -> bool {
        self.0.try_decr()
    }
}

/// Recorder for [`InFlight`]: decrements the gauge exactly once when finished,
/// balancing the increment performed on entry.
#[derive(Debug)]
pub struct InFlightRecorder {
    gauge: Handle<AtomicI64>,
    armed: Armed,
}

impl Drop for InFlightRecorder {
    fn drop(&mut self) {
        if self.armed.fire() {
            self.gauge.decr();
        }
    }
}

impl Measure for InFlight {
    type Recorder = InFlightRecorder;

    fn enter(&self) -> InFlightRecorder {
        self.0.incr();
        InFlightRecorder {
            gauge: self.0.share(),
            armed: Armed::new(),
        }
    }
}

impl<R> Recorder<R> for InFlightRecorder {
    fn complete(&mut self, _result: &R) {
        if self.armed.fire() {
            self.gauge.decr();
        }
    }
}

impl Deref for InFlight {
    type Target = AtomicI64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
