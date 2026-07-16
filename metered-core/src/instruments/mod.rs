//! The concrete OpenMetrics instruments and the `Histogram` trait over them.

mod atomic_f64;

pub mod bucket_histogram;
pub mod exponential_histogram;
pub mod gauge_histogram;
pub mod histogram;
pub mod primitives;
pub mod summary;
