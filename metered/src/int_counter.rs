use crate::metric::Counter;
use atomic::Atomic;
use std::cell::Cell;
use std::sync::atomic::Ordering;

macro_rules! impl_counter_for {
    ($int:path) => {
        impl Counter for Cell<$int> {
            fn incr(&self) {
                self.set(self.get() + 1);
            }
        }

        impl Counter for Atomic<$int> {
            fn incr(&self) {
                self.fetch_add(1, Ordering::Relaxed);
            }
        }
    };
}

impl_counter_for!(u8);
impl_counter_for!(u16);
impl_counter_for!(u32);
impl_counter_for!(u64);
impl_counter_for!(u128);
impl_counter_for!(usize);
