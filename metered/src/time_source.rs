//! A module for Time Sources.

use std::{convert::TryFrom, time::Duration};

/// A trait for any time source providing time measurements in milliseconds.
///
/// It is useful to let users provide an unsynchronized  (`!Send`/`!Sync`) time
/// source, unlike std's `Instant`.
pub trait Instant {
    /// Creates a new Instant representing the current time.
    fn now() -> Self;

    /// Returns the elapsed time since an Instant was created.
    ///
    /// The unit depends on the Instant's resolution, as defined by the
    /// `ONE_SEC` constant.
    fn elapsed_time(&self) -> u64;

    /// Return the amount of units from a Duration.
    fn units(duration: Duration) -> u64;

    /// One second in the instant units.
    const ONE_SEC: u64;
}

/// A new-type wrapper for std Instants and Metered's
/// [Instant] trait that measures time in milliseconds.
#[derive(Debug, Clone)]
pub struct StdInstant(std::time::Instant);
impl Instant for StdInstant {
    const ONE_SEC: u64 = 1_000;

    fn now() -> Self {
        StdInstant(std::time::Instant::now())
    }

    fn elapsed_time(&self) -> u64 {
        let elapsed = self.0.elapsed();

        elapsed.as_secs() * Self::ONE_SEC + u64::from(elapsed.subsec_millis())
    }

    fn units(duration: Duration) -> u64 {
        u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
    }
}

/// A new-type wrapper for std Instants and Metered's
/// [Instant] trait that measures time in microseconds.
#[derive(Debug, Clone)]
pub struct StdInstantMicros(std::time::Instant);
impl Instant for StdInstantMicros {
    const ONE_SEC: u64 = 1_000_000;

    fn now() -> Self {
        StdInstantMicros(std::time::Instant::now())
    }

    fn elapsed_time(&self) -> u64 {
        let elapsed = self.0.elapsed();

        elapsed.as_secs() * Self::ONE_SEC + u64::from(elapsed.subsec_micros())
    }

    fn units(duration: Duration) -> u64 {
        u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
    }
}
