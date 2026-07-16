use crate::handle::Handle;
use std::sync::atomic::{AtomicU64, Ordering};

/// A readable counter value: the read-only side of [`Counter`].
///
/// Exposition only ever needs this trait -- a registry entry samples `get` at
/// scrape time. Read-only adapters (e.g. [`crate::adapter::CounterFn`], which
/// reads a total the application already maintains) implement `CounterSource`
/// alone, so they can never be handed out where mutation is expected.
pub trait CounterSource {
    /// Returns the current count.
    fn get(&self) -> u64;
}

/// A monotonically increasing counter instrument.
///
/// Extends [`CounterSource`] with mutation. Implement it only for values the
/// metric owner may actually increment; a read-only adapter stays a
/// `CounterSource`.
///
/// # `incr` ambiguity with [`Gauge`](crate::Gauge)
///
/// `AtomicU64` implements both `Counter` and [`Gauge`](crate::Gauge), and both
/// traits have an `incr` method. With **both traits in scope**, a plain
/// `value.incr()` on an `AtomicU64` is ambiguous and fails to compile; pick
/// the intended trait with fully-qualified syntax:
///
/// ```
/// use metered::{Counter, Gauge};
/// use std::sync::atomic::AtomicU64;
///
/// let requests = AtomicU64::new(0);
/// Counter::incr(&requests); // the counter reading
/// Gauge::incr(&requests);   // the gauge reading
/// ```
///
/// With only one of the traits in scope, `requests.incr()` resolves normally.
pub trait Counter: CounterSource {
    /// Increments the counter by one.
    fn incr(&self) {
        self.incr_by(1);
    }

    /// Increments the counter by `n`.
    fn incr_by(&self, n: u64);
}

impl CounterSource for AtomicU64 {
    fn get(&self) -> u64 {
        self.load(Ordering::Relaxed)
    }
}

/// `AtomicU64` as a counter.
///
/// `AtomicU64` also implements [`Gauge`](crate::Gauge), so when both traits
/// are in scope call `Counter::incr(&value)` (see the [`Counter`] trait docs
/// on the `incr` ambiguity).
impl Counter for AtomicU64 {
    fn incr_by(&self, n: u64) {
        self.fetch_add(n, Ordering::Relaxed);
    }
}

impl<T: CounterSource + ?Sized> CounterSource for &T {
    fn get(&self) -> u64 {
        (**self).get()
    }
}

impl<T: Counter + ?Sized> Counter for &T {
    fn incr_by(&self, n: u64) {
        (**self).incr_by(n);
    }
}

impl<T: CounterSource> CounterSource for Handle<T> {
    fn get(&self) -> u64 {
        (**self).get()
    }
}

impl<T: Counter> Counter for Handle<T> {
    fn incr_by(&self, n: u64) {
        (**self).incr_by(n);
    }
}

/// Explicitly exposes a value as a counter metric.
#[derive(Clone, Copy, Debug)]
pub struct AsCounter<T>(pub T);

impl<T> From<T> for AsCounter<T> {
    fn from(counter: T) -> Self {
        AsCounter(counter)
    }
}

impl<T: CounterSource> CounterSource for AsCounter<T> {
    fn get(&self) -> u64 {
        self.0.get()
    }
}

impl<T: Counter> Counter for AsCounter<T> {
    fn incr_by(&self, n: u64) {
        self.0.incr_by(n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_is_readable_source_of_truth() {
        let c = AtomicU64::new(0);
        c.incr();
        c.incr_by(4);
        assert_eq!(CounterSource::get(&c), 5);
    }

    #[test]
    fn counter_trait_name_stays_public_and_as_counter_wraps_it() {
        let c = AtomicU64::new(0);
        Counter::incr(&c);
        Counter::incr_by(&c, 4);
        assert_eq!(CounterSource::get(&c), 5);

        let wrapped = AsCounter::from(&c);
        wrapped.incr();
        assert_eq!(wrapped.get(), 6);
    }
}
