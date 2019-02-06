//! A module providing a Clear trait which signals metrics to clear their state if applicable.

/// The `Clear` trait is used to signal metrics to clear their state if applicable
///
/// While it is recommended all metrics should implement `Clear`, for instance to derive `Clear`
/// on registries, some metrics may choose to do nothing. For instance, Gauges would be left in an inconsistent state
/// if they were altered during clear.
pub trait Clear {
    /// Requests to clear self.
    fn clear(&self);
}
