use crate::handle::Handle;
use crate::Scalar;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};

/// A readable gauge value: the read-only side of [`Gauge`].
///
/// Exposition only ever needs this trait -- a registry entry samples `get` at
/// scrape time. Read-only adapters (e.g. [`crate::adapter::GaugeFn`], which
/// reads a value the application already owns) implement `GaugeSource` alone,
/// so they can never be handed out where mutation is expected.
pub trait GaugeSource {
    /// The numeric value type used by this gauge.
    type Value: Copy + Into<Scalar>;

    /// Returns the current value.
    fn get(&self) -> Self::Value;

    /// Returns `true` if the gauge is non-zero (its boolean reading).
    fn is_set(&self) -> bool
    where
        Self::Value: Default + PartialEq,
    {
        self.get() != Self::Value::default()
    }
}

/// A gauge instrument that can move up and down.
///
/// Extends [`GaugeSource`] with mutation. Implement it only for values the
/// metric owner may actually move; a read-only adapter stays a `GaugeSource`.
pub trait Gauge: GaugeSource {
    /// Sets the gauge to `value`.
    fn set(&self, value: Self::Value);

    /// Adds `delta` to the gauge.
    fn add(&self, delta: Self::Value);

    /// Increments the gauge by one.
    fn incr(&self);

    /// Attempts to decrement the gauge by one.
    fn try_decr(&self) -> bool;

    /// Decrements the gauge by one, saturating at the value-domain floor.
    ///
    /// A gauge underflow -- e.g. a double-decrement of an unsigned gauge at
    /// zero, typically from a `Drop`-based accounting bug -- must never take the
    /// process down: recorders decrement gauges in `Drop`, and a panic during
    /// unwind aborts the process outright. So the default clamps by ignoring a
    /// failed [`try_decr`](Gauge::try_decr), with only a `debug_assert!` to
    /// surface the bug in development. Callers who need the signal in release
    /// builds call [`try_decr`](Gauge::try_decr) directly.
    fn decr(&self) {
        let decremented = self.try_decr();
        debug_assert!(
            decremented,
            "gauge decrement would leave the value domain (saturated instead)"
        );
    }

    /// Sets the gauge to `1` for `true` and `0` for `false`.
    fn set_enabled(&self, enabled: bool)
    where
        Self::Value: From<u8>,
    {
        self.set(u8::from(enabled).into());
    }
}

impl GaugeSource for AtomicI64 {
    type Value = i64;

    fn get(&self) -> Self::Value {
        self.load(Ordering::Relaxed)
    }
}

impl Gauge for AtomicI64 {
    fn set(&self, value: Self::Value) {
        self.store(value, Ordering::Relaxed);
    }

    fn add(&self, delta: Self::Value) {
        self.fetch_add(delta, Ordering::Relaxed);
    }

    fn incr(&self) {
        self.add(1);
    }

    fn try_decr(&self) -> bool {
        self.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_sub(1)
        })
        .is_ok()
    }
}

impl GaugeSource for AtomicU64 {
    type Value = u64;

    fn get(&self) -> Self::Value {
        self.load(Ordering::Relaxed)
    }
}

impl Gauge for AtomicU64 {
    fn set(&self, value: Self::Value) {
        self.store(value, Ordering::Relaxed);
    }

    fn add(&self, delta: Self::Value) {
        self.fetch_add(delta, Ordering::Relaxed);
    }

    fn incr(&self) {
        self.add(1);
    }

    fn try_decr(&self) -> bool {
        self.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_sub(1)
        })
        .is_ok()
    }
}

impl GaugeSource for AtomicUsize {
    type Value = usize;

    fn get(&self) -> Self::Value {
        self.load(Ordering::Relaxed)
    }
}

impl Gauge for AtomicUsize {
    fn set(&self, value: Self::Value) {
        self.store(value, Ordering::Relaxed);
    }

    fn add(&self, delta: Self::Value) {
        self.fetch_add(delta, Ordering::Relaxed);
    }

    fn incr(&self) {
        self.add(1);
    }

    fn try_decr(&self) -> bool {
        self.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_sub(1)
        })
        .is_ok()
    }
}

impl<T: GaugeSource + ?Sized> GaugeSource for &T {
    type Value = T::Value;

    fn get(&self) -> Self::Value {
        (**self).get()
    }
}

impl<T: Gauge + ?Sized> Gauge for &T {
    fn set(&self, value: Self::Value) {
        (**self).set(value);
    }

    fn add(&self, delta: Self::Value) {
        (**self).add(delta);
    }

    fn incr(&self) {
        (**self).incr();
    }

    fn try_decr(&self) -> bool {
        (**self).try_decr()
    }
}

impl<T: GaugeSource> GaugeSource for Handle<T> {
    type Value = T::Value;

    fn get(&self) -> Self::Value {
        (**self).get()
    }
}

impl<T: Gauge> Gauge for Handle<T> {
    fn set(&self, value: Self::Value) {
        (**self).set(value);
    }

    fn add(&self, delta: Self::Value) {
        (**self).add(delta);
    }

    fn incr(&self) {
        (**self).incr();
    }

    fn try_decr(&self) -> bool {
        (**self).try_decr()
    }
}

/// Explicitly exposes a value as a gauge metric.
#[derive(Clone, Copy, Debug)]
pub struct AsGauge<T>(pub T);

impl<T> From<T> for AsGauge<T> {
    fn from(gauge: T) -> Self {
        AsGauge(gauge)
    }
}

impl<T: GaugeSource> GaugeSource for AsGauge<T> {
    type Value = T::Value;

    fn get(&self) -> Self::Value {
        self.0.get()
    }
}

impl<T: Gauge> Gauge for AsGauge<T> {
    fn set(&self, value: Self::Value) {
        self.0.set(value);
    }

    fn add(&self, delta: Self::Value) {
        self.0.add(delta);
    }

    fn incr(&self) {
        self.0.incr();
    }

    fn try_decr(&self) -> bool {
        self.0.try_decr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_is_a_readable_source_of_truth() {
        let enabled = AtomicI64::new(0);
        enabled.set_enabled(true);
        assert!(enabled.is_set());
        enabled.set_enabled(false);
        assert!(!enabled.is_set());

        let depth = AtomicI64::new(0);
        depth.incr();
        depth.incr();
        depth.decr();
        assert_eq!(GaugeSource::get(&depth), 1);
    }

    #[test]
    fn gauge_trait_name_stays_public_and_supports_signed_values() {
        let depth = AtomicI64::new(0);
        Gauge::incr(&depth);
        Gauge::incr(&depth);
        Gauge::decr(&depth);
        assert_eq!(GaugeSource::get(&depth), 1);
    }

    #[test]
    fn gauge_supports_unsigned_values_and_as_gauge_wraps_it() {
        let depth = AtomicU64::new(0);
        Gauge::set(&depth, 3);
        Gauge::incr(&depth);
        assert_eq!(GaugeSource::get(&depth), 4);
        assert!(Gauge::try_decr(&depth));
        assert_eq!(GaugeSource::get(&depth), 3);

        let wrapped = AsGauge::from(&depth);
        wrapped.incr();
        assert_eq!(wrapped.get(), 4);
    }
}
