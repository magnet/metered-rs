//! Compile-checked API samples.
//!
//! Each sample is a standalone `.rs` file in the crate root, included here as
//! a module so `cargo build --workspace` (and therefore CI) fails the moment a
//! sample teaches an API that no longer exists. Samples demonstrate call
//! shapes rather than runnable programs, so unused items are expected.

#![allow(dead_code)]

#[path = "../counter_gauge.rs"]
mod counter_gauge;
#[path = "../exponential_histogram.rs"]
mod exponential_histogram;
#[path = "../family_labels.rs"]
mod family_labels;
#[path = "../family_view.rs"]
mod family_view;
#[path = "../grpc_semconv_profile.rs"]
mod grpc_semconv_profile;
#[path = "../metric_tree_derive.rs"]
mod metric_tree_derive;
#[path = "../metric_view_context.rs"]
mod metric_view_context;
#[path = "../tracing_span_metrics.rs"]
mod tracing_span_metrics;
