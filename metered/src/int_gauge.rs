//! A module providing thread-safe and unsynchronized implementations for Gauges
//! on various unsized integers.

use crate::{
    atomic::AtomicInt,
    metric::{BatchGauge, Gauge},
};
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

        impl BatchGauge for Cell<$int> {
            fn incr_by(&self, count: usize) {
                let num = count as $int;
                self.set(self.get() + num);
            }

            fn decr_by(&self, count: usize) {
                let num = count as $int;
                self.set(self.get() - num);
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

        impl BatchGauge for AtomicInt<$int> {
            fn incr_by(&self, count: usize) {
                let num = count as $int;
                AtomicInt::<$int>::incr_by(&self, num);
            }

            fn decr_by(&self, count: usize) {
                let num = count as $int;
                AtomicInt::<$int>::decr_by(&self, num);
            }
        }
    };
}

impl_gauge_for!(u8);
impl_gauge_for!(u16);
impl_gauge_for!(u32);
impl_gauge_for!(u64);
impl_gauge_for!(u128);
