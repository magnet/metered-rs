//! A module providing thread-safe and unsynchronized implementations for
//! Counters on various unsized integers.

use crate::{
    atomic::AtomicInt,
    clear::{Clear, Clearable},
    metric::Counter,
    num_wrapper::NumWrapper,
};
use std::cell::Cell;

macro_rules! impl_counter_for {
    ($int:path) => {
        impl Counter for Cell<$int> {
            fn incr_by(&self, count: usize) {
                let v = NumWrapper::<$int>::wrap(count);
                self.set(self.get().wrapping_add(v));
            }
        }

        impl Clear for Cell<$int> {
            fn clear(&self) {
                self.set(0);
            }
        }

        impl Clearable for Cell<$int> {
            fn is_cleared(&self) -> bool {
                self.get() == 0
            }
        }

        impl Counter for AtomicInt<$int> {
            fn incr_by(&self, count: usize) {
                let v = NumWrapper::<$int>::wrap(count);
                AtomicInt::<$int>::incr_by(&self, v);
            }
        }

        impl Clear for AtomicInt<$int> {
            fn clear(&self) {
                AtomicInt::<$int>::set(&self, 0);
            }
        }

        impl Clearable for AtomicInt<$int> {
            fn is_cleared(&self) -> bool {
                AtomicInt::<$int>::get(&self) == 0
            }
        }
    };
}

impl_counter_for!(u8);
impl_counter_for!(u16);
impl_counter_for!(u32);
impl_counter_for!(u64);
impl_counter_for!(u128);
