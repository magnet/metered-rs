//! Procedural macros for Metered, a metric library for Rust.
//!
//! Please check the Metered crate for more documentation.

// The `quote!` macro requires deep recursion.
#![recursion_limit = "512"]

#[macro_use]
extern crate syn;
#[macro_use]
extern crate quote;

// Core: the modern derives (MetricTree / LabelSet / SpanLabels) plus the shared
// `#[metric]` / `#[metrics]` attribute parser they (and the legacy macros) use.
mod derive;
mod metric_attr;

use proc_macro::TokenStream;

/// Derives `metered::LabelSet` for a struct: each named field becomes a label
/// (`field_name = field.to_string()`), for use as a typed `Family` key.
#[proc_macro_derive(LabelSet)]
pub fn derive_label_set(input: TokenStream) -> TokenStream {
    derive::label_set(input)
}

/// Derives `metered::MetricTree` for a struct of metrics.
///
/// Fields are **opt-in**: only fields carrying a `#[metrics]` attribute are
/// emitted. This lets service structs contain non-metric configuration/state
/// without needing a "skip" attribute.
///
/// Container-level `#[metrics(...)]` attributes shape the whole tree:
/// - `#[metrics(prefix = "wire_prefix")]` joins `wire_prefix` onto the inherited
///   metric name before fields are emitted.
/// - `#[metrics(label(key = "value"))]` adds a constant label to every family
///   emitted by the tree.
/// - `#[metrics(help = "HELP text")]` exposes default root HELP metadata through
///   `MetricTreeMeta`.
/// - `#[metrics(unit = "items")]` exposes default root UNIT metadata through
///   `MetricTreeMeta`.
///
/// Per-field `#[metrics(...)]` / `#[metric(...)]` attributes opt a field into the tree and control
/// the emitted shape:
/// - Bare `#[metrics]` exposes a component through its `MetricsView`.
/// - Bare `#[metric]` exposes a metric-family leaf through its own `MetricTree` impl.
/// - `#[metric(rename = "wire_name")]` emits the metric under `wire_name` instead
///   of the Rust field name.
/// - `#[metrics(flatten)]` drops the field's name segment, so the nested tree's
///   children sit at the parent level (the metric-tree analogue of
///   `#[serde(flatten)]`).
/// - `#[metrics(info)]` exposes an `Info` value.
/// - `#[metric(gauge)]` / `#[metric(counter)]` disambiguate primitive leaves.
#[proc_macro_derive(MetricTree, attributes(metric, metrics))]
pub fn derive_metric_tree(input: TokenStream) -> TokenStream {
    derive::metric_tree(input)
}
