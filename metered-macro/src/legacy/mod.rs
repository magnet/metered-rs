//! Legacy method-instrumentation macros for the `metered-semantic` crate.
//!
//! `#[metered]`, `#[measure]`, and `#[error_count]` are the metered-0.9
//! "annotate a method, auto-collect" ergonomic. They generate code against
//! `::metered_semantic::` (never `::metered::`), and nothing in metered's core
//! -- the `MetricTree` / `LabelSet` / `SpanLabels` derives -- depends on this
//! module. It is kept only for back-compat and is scheduled for removal:
//! deleting this `legacy/` directory plus the two `#[proc_macro_attribute]`
//! entry points in `lib.rs` drops the whole feature.

pub mod error_count;
pub mod metered;

mod error_count_opts;
mod measure_opts;
mod metered_opts;
