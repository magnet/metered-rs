pub mod atomic;
pub mod clear;
pub mod common;
pub mod hdr_histogram;
pub mod int_counter;
pub mod int_gauge;
pub mod metric;
pub mod time_source;

pub use common::{ErrorCount, HitCount, InFlight, ResponseTime, Throughput};
pub use metered_macro::metered;

// Re-export this type so 3rd-party crates don't need to depend on the `aspect-rs` crate.
pub use aspect::{Advice, Enter};

aspect::define!(measure: metered::metric::on_result);
