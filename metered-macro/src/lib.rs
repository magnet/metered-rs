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

// Legacy method-instrumentation macros (`#[metered]` / `#[measure]` /
// `#[error_count]`) for `metered-semantic`. Self-contained and scheduled for
// removal -- see `legacy/mod.rs`.
mod legacy;

use proc_macro::TokenStream;

/// A procedural macro that generates a metric registry for an `impl` block.
///
/// Generated registries implement `metered::MetricTree` for OpenMetrics-style
/// exposition. They do not derive `serde::Serialize`.
///
/// ```
/// use metered_semantic::{metered, Elapsed, HitCount};
///
/// #[derive(Default, Debug)]
/// pub struct Biz {
///     metrics: BizMetrics,
/// }
///
/// #[metered_semantic::metered(registry = BizMetrics)]
/// impl Biz {
///     #[measure([HitCount, Elapsed])]
///     pub fn biz(&self) {        
///         let delay = std::time::Duration::from_millis(rand::random::<u64>() % 200);
///         std::thread::sleep(delay);
///     }   
/// }
/// #
/// # let biz = Biz::default();
/// # biz.biz();
/// # assert_eq!(biz.metrics.biz.hit_count.get(), 1);
/// ```
///
/// ### The `metered` attribute
///
/// `#[metered(registry = YourRegistryName, registry_expr =
/// self.wrapper.my_registry)]`
///
/// `registry` is mandatory and must be a valid Rust ident.
///
/// `registry_expr` defaults to `self.metrics`, alternate values must be a valid
/// Rust expression.
///
/// ### The `measure` attribute
///
/// Single metric:
///
/// `#[measure(path::to::MyMetric<u64>)]`
///
/// or:
///
/// `#[measure(type = path::to::MyMetric<u64>)]`
///
/// Multiple metrics:
///
/// `#[measure([path::to::MyMetric<u64>, path::AnotherMetric])]`
///
/// or
///
/// `#[measure(type = [path::to::MyMetric<u64>, path::AnotherMetric])]`
///
/// The `type` keyword is allowed because other keywords are planned for future
/// extra attributes (e.g, instantation options).
///
/// When `measure` attribute is applied to an `impl` block, it applies for every
/// method that has a `measure` attribute. If a method does not need extra
/// measure infos, it is possible to annotate it with simply `#[measure]` and
/// the `impl` block's `measure` configuration will be applied.
///
/// The `measure` keyword can be added several times on an `impl` block or
/// method, which will add to the list of metrics applied. Adding the same
/// metric several time will lead in a name clash.
///
/// ### The `metric` attribute
///
/// `#[metric(rename = "wire_name")]` on a measured method controls the wire-name
/// segment that method contributes, independently of the Rust method name. The
/// generated registry field keeps the method name, so renaming the method in
/// code does not move its metrics.
#[proc_macro_attribute]
pub fn metered(attrs: TokenStream, item: TokenStream) -> TokenStream {
    legacy::metered::metered(attrs, item)
        .unwrap_or_else(|e| TokenStream::from(e.to_compile_error()))
}

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

/// A procedural macro that generates a new metric that measures the amount
/// of times each variant of an error has been thrown, to be used as
/// crate-specific replacement for `metered_semantic::ErrorCount`.
///
/// ```
/// # use metered::CounterSource;
/// # use metered_semantic::{metered, error_count};
/// # use thiserror::Error;
/// #
/// #[error_count(name = LibErrorCount, visibility = pub)]
/// #[derive(Debug, Error)]
/// pub enum LibError {
/// #   #[error("read error")]
///     ReadError,
/// #   #[error("init error")]
///     InitError,
/// }
///
/// #[error_count(name = ErrorCount, visibility = pub)]
/// #[derive(Debug, Error)]
/// pub enum Error {
/// #   #[error("error from lib: {0}")]
///     MyLibrary(#[from] #[nested] LibError),
/// }
///
/// #[derive(Default, Debug)]
/// pub struct Baz {
///     metrics: BazMetrics,
/// }
///
/// #[metered(registry = BazMetrics)]
/// impl Baz {
///     #[measure(ErrorCount)]
///     pub fn biz(&self) -> Result<(), Error> {        
///         Err(LibError::InitError.into())
///     }   
/// }
///
/// let baz = Baz::default();
/// baz.biz();
/// assert_eq!(baz.metrics.biz.error_count.my_library.read_error.get(), 0);
/// assert_eq!(baz.metrics.biz.error_count.my_library.init_error.get(), 1);
/// ```
///
/// - `name` is required and must be a valid Rust ident, this is the name of the
///   generated struct containing a counter for each enum variant.
/// - `visibility` specifies to visibility of the generated struct, it defaults
///   to `pub(crate)`.
///
/// The `error_count` macro may only be applied to any enums that have a
/// `std::error::Error` impl. The generated struct may then be included
/// in `measure` attributes to measure the amount of errors returned of
/// each variant defined in your error enum.
#[proc_macro_attribute]
pub fn error_count(attrs: TokenStream, item: TokenStream) -> TokenStream {
    legacy::error_count::error_count(attrs, item)
        .unwrap_or_else(|e| TokenStream::from(e.to_compile_error()))
}
