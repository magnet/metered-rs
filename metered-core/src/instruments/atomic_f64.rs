//! A lock-free `f64` accumulator, shared by the histogram backends for their
//! running `sum`.

use std::sync::atomic::{AtomicU64, Ordering};

/// An `f64` updated atomically via a compare-and-swap loop over its bit pattern.
///
/// The observe path adds to it lock-free; scrapes read it. All operations use
/// `Relaxed` ordering -- the sum is monitoring data, consistent "enough" with
/// the buckets, not a synchronization point.
#[derive(Debug)]
pub(crate) struct AtomicF64(AtomicU64);

impl AtomicF64 {
    /// Creates an accumulator initialized to `value`.
    pub(crate) fn new(value: f64) -> Self {
        AtomicF64(AtomicU64::new(value.to_bits()))
    }

    /// Atomically adds `value` to the accumulator.
    pub(crate) fn add(&self, value: f64) {
        let mut current = self.0.load(Ordering::Relaxed);
        loop {
            let updated = (f64::from_bits(current) + value).to_bits();
            match self.0.compare_exchange_weak(
                current,
                updated,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    /// Loads the current value.
    pub(crate) fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
}
