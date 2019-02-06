//! A module for Time Sources.

/// A trait for any time source providing time measurements in milliseconds.
///
/// It is useful to let users provide an unsynchronized  (`!Send`/`!Sync`) time source, unlike std's `Instant`.
pub trait Instant {
    /// Creates a new Instant representing the current time.
    fn now() -> Self;

    /// Returns the elapsed time in milliseconds since an Instant was created.
    fn elapsed_millis(&self) -> u64;
}

/// A new-type wrapper for std Instants and Metered's [Instant](trait.Instant.html) trait.
#[derive(Debug, Clone)]
pub struct StdInstant(std::time::Instant);
impl Instant for StdInstant {
    fn now() -> Self {
        StdInstant(std::time::Instant::now())
    }

    fn elapsed_millis(&self) -> u64 {
        let elapsed = self.0.elapsed();

        elapsed.as_secs() * 1000 + (elapsed.subsec_nanos() / 1_000_000) as u64
    }
}
