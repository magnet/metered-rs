use crate::atomic::AtomicInt;
use crate::metric::Counter;
use std::cell::Cell;

macro_rules! impl_counter_for {
    ($int:path) => {
        impl Counter for Cell<$int> {
            fn incr(&self) {
                self.set(self.get() + 1);
            }
        }

        impl Counter for AtomicInt<$int> {
            fn incr(&self) {
                AtomicInt::<$int>::incr(&self);
            }
        }
    };
}

impl_counter_for!(u8);
impl_counter_for!(u16);
impl_counter_for!(u32);
impl_counter_for!(u64);
impl_counter_for!(u128);
