//! A module providing new-type Atomic wrapper that implements Debug &
//! Serialize.

use serde::{Serialize, Serializer};
use std::{
    fmt,
    fmt::{Debug, Display},
    sync::atomic::Ordering,
};

/// A new-type wrapper over `atomic::Atomic` that supports serde serialization
/// and a cleaner debug output.
///
/// All default operations on the wrapper type are using a relaxed memory
/// ordering, which makes it suitable for counters and little else.
#[derive(Default)]
pub struct AtomicInt<T: Copy> {
    /// The inner atomic instance
    pub inner: atomic::Atomic<T>,
}

impl<T: Copy> AtomicInt<T> {
    /// Returns the current value with a relaxed
    pub fn get(&self) -> T {
        self.inner.load(Ordering::Relaxed)
    }
}

impl<T: Copy + Display> Debug for AtomicInt<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.get())
    }
}

macro_rules! impl_blocks_for {
    ($int:path: $method_name:ident) => {
        impl AtomicInt<$int> {
            /// Increments self
            pub fn incr(&self) -> $int {
                self.inner.fetch_add(1, Ordering::Relaxed)
            }

            /// Increments self by count
            pub fn incr_by(&self, count: $int) -> $int {
                self.inner.fetch_add(count, Ordering::Relaxed)
            }

            /// Decrements self
            pub fn decr(&self) -> $int {
                self.inner.fetch_sub(1, Ordering::Relaxed)
            }

            /// Decrements self by count
            pub fn decr_by(&self, count: $int) -> $int {
                self.inner.fetch_sub(count, Ordering::Relaxed)
            }

            /// Sets self to a new value
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
