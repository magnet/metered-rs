//! A module providing thread-safe and unsynchronized implementations for Gauges
//! on various unsized integers.

use crate::{atomic::AtomicInt, metric::Gauge, num_wrapper::NumWrapper};
use std::cell::Cell;

macro_rules! impl_gauge_for {
    ($int:path) => {
        impl Gauge for Cell<$int> {
            fn incr_by(&self, count: usize) {
                let v = NumWrapper::<$int>::wrap(count);
                self.set(self.get().wrapping_add(v));
            }

            fn decr_by(&self, count: usize) {
                let v = NumWrapper::<$int>::wrap(count);
                self.set(self.get().wrapping_sub(v));
            }
        }

        impl Gauge for AtomicInt<$int> {
            fn incr_by(&self, count: usize) {
                let v = NumWrapper::<$int>::wrap(count);
                AtomicInt::<$int>::incr_by(&self, v);
            }

            fn decr_by(&self, count: usize) {
                let v = NumWrapper::<$int>::wrap(count);
                AtomicInt::<$int>::decr_by(&self, v);
            }
        }
    };
}

impl_gauge_for!(u8);
impl_gauge_for!(u16);
impl_gauge_for!(u32);
impl_gauge_for!(u64);
impl_gauge_for!(u128);
