//! A module providing thread-safe and unsynchronized implementations for Gauges on various unsized integers.

use crate::atomic::AtomicInt;
use crate::metric::Gauge;
use std::cell::Cell;

macro_rules! impl_gauge_for {
    ($int:path) => {
        impl Gauge for Cell<$int> {
            fn incr(&self) {
                self.set(self.get() + 1);
            }

            fn decr(&self) {
                self.set(self.get() - 1);
            }
        }

        impl Gauge for AtomicInt<$int> {
            fn incr(&self) {
                AtomicInt::<$int>::incr(&self);
            }

            fn decr(&self) {
                AtomicInt::<$int>::decr(&self);
            }
        }
    };
}

impl_gauge_for!(u8);
impl_gauge_for!(u16);
impl_gauge_for!(u32);
impl_gauge_for!(u64);
impl_gauge_for!(u128);
