use crate::metric::Gauge;
use atomic::Atomic;
use std::cell::Cell;
use std::sync::atomic::Ordering;

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

        impl Gauge for Atomic<$int> {
            fn incr(&self) {
                self.fetch_add(1, Ordering::Relaxed);
            }

            fn decr(&self) {
                self.fetch_sub(1, Ordering::Relaxed);
            }
        }
    };
}

impl_gauge_for!(u8);
impl_gauge_for!(u16);
impl_gauge_for!(u32);
impl_gauge_for!(u64);
impl_gauge_for!(u128);
impl_gauge_for!(usize);
