//! A module providing new-type Atomic wrapper that implements Debug & Serialize.

use serde::{Serialize, Serializer};
use std::fmt;
use std::fmt::{Debug, Display};
use std::sync::atomic::Ordering;

#[derive(Default)]
pub struct AtomicInt<T: Copy> {
    pub inner: atomic::Atomic<T>,
}

impl<T: Copy> AtomicInt<T> {
    pub fn get(&self) -> T {
        self.inner.load(Ordering::Relaxed)
    }
}

impl<T: Copy + Display> Debug for AtomicInt<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.get())
    }
}

macro_rules! impl_blocks_for {
    ($int:path: $method_name:ident) => {
        impl AtomicInt<$int> {
            pub fn incr(&self) -> $int {
                self.inner.fetch_add(1, Ordering::Relaxed)
            }

            pub fn decr(&self) -> $int {
                self.inner.fetch_sub(1, Ordering::Relaxed)
            }

            pub fn set(&self, v: $int) {
                self.inner.store(v, Ordering::Relaxed);
            }
        }

        impl Serialize for AtomicInt<$int> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.$method_name(self.get())
            }
        }
    };
}

impl_blocks_for!(u8: serialize_u8);
impl_blocks_for!(u16: serialize_u16);
impl_blocks_for!(u32: serialize_u32);
impl_blocks_for!(u64: serialize_u64);
impl_blocks_for!(u128: serialize_u128);
