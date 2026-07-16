//! Procedural macros for Metered, a metric library for Rust.
//!
//! Please check the Metered crate for more documentation.

// The `quote!` macro requires deep recursion.
#![recursion_limit = "512"]

// The modern derives (MetricTree / LabelSet / SpanLabels) plus the shared
// `#[metric]` / `#[metrics]` attribute parser they use.
mod derive;
mod metric_attr;
mod span_metric;

use proc_macro::TokenStream;

/// Derives `metered::LabelSet` for a struct: each named field becomes a label
/// (`field_name = field.to_string()`), for use as a typed `Family` key.
///
/// Fields whose type is one of the standard string shapes -- `String`,
/// `&str` (any lifetime), `Arc<str>`, `Cow<'_, str>` -- **lend** their stored
/// text to the `for_each_label` visitor directly instead of allocating a
/// fresh `String` through `ToString`, so encoding a string-labeled key is
/// allocation-free. The rendered bytes are identical either way. The check
/// is textual (a proc macro cannot resolve types), so an aliased or
/// unrecognized string type simply keeps the `ToString` path.
#[proc_macro_derive(LabelSet, attributes(metrics))]
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
/// - `#[metrics(crate = "...")]` overrides the emitted runtime path (default
///   `::metered`), for renamed facades or local re-exports. The value is a
///   string-literal path, mirroring `#[serde(crate = "...")]` /
///   `#[span(crate = "...")]`. `#[derive(LabelSet)]` reads the same option
///   from `#[metrics(...)]`.
///
/// Per-field `#[metrics(...)]` / `#[metric(...)]` attributes opt a field into the tree and control
/// the emitted shape:
/// - Bare `#[metrics]` exposes a component through its `MetricsView`. The
///   component's view layout is built at most once per process and cached in
///   a hidden `static` (one per view field), so scrapes never rebuild it. A
///   `static` cannot mention generic parameters, so when the field's type
///   involves the deriving struct's generics the derive falls back to
///   rebuilding the view on every call for that field.
/// - Bare `#[metric]` exposes a metric-family leaf through its own `MetricTree` impl.
/// - `#[metric(rename = "wire_name")]` emits the metric under `wire_name` instead
///   of the Rust field name.
/// - `#[metrics(flatten)]` drops the field's name segment, so the nested tree's
///   children sit at the parent level (the metric-tree analogue of
///   `#[serde(flatten)]`).
/// - `#[metrics(info)]` exposes an `Info` value through the public `AsInfo`
///   adapter (same leaf path as `#[metric(gauge)]` / `#[metric(counter)]`
///   with `AsGauge` / `AsCounter`).
/// - `#[metric(gauge)]` / `#[metric(counter)]` disambiguate primitive leaves.
#[proc_macro_derive(MetricTree, attributes(metric, metrics))]
pub fn derive_metric_tree(input: TokenStream) -> TokenStream {
    derive::metric_tree(input)
}

/// Derives the span/metric wiring for a typed labels struct.
///
/// Re-exported as `metered_tracing::SpanLabels`; see that crate for usage. Pair
/// it with `#[derive(LabelSet)]`: the struct fields are the OpenMetrics labels
/// (their types are enforced), and `#[span("otel.field")]` maps each to the
/// OpenTelemetry semconv span field it reads from. Generates `FromSpanFields`
/// (parsing each value with `FromStr`), the `@open` arm reached through
/// `metered_info_span!`, typed `record_*` setters for `on_close` fields, and
/// `SPAN` / `HELP` consts -- so the emitted span field and the metric label,
/// keyed by the same struct, cannot drift. The metrics themselves are normal
/// `metered` families you own (a counter you bump, a duration histogram a
/// `SpanDurations` adapter times).
///
/// Each field type must be `FromStr + Default` (`Default` is the last-resort
/// fallback when a span omits the field; `from_span_fields` never panics).
/// Generated code reaches `tracing` through `metered_tracing`'s re-export, so a
/// deriving crate needs no direct `tracing` dependency.
///
/// The `@open` macro is `#[macro_export]`ed under the labels type's name, so it
/// is reachable from any module (and before its definition), not only where the
/// struct is declared.
///
/// # Exported opener name collisions (`macro_name`)
///
/// Because the `@open` opener is `#[macro_export]`ed, it always lands at the
/// **crate root** regardless of the module the type is declared in. Two
/// same-named labels types in different modules would therefore export two
/// crate-root macros with one name, which fails to compile ("the name `...`
/// is defined multiple times"). The `#[span(macro_name = "...")]` container
/// attribute renames one type's opener; open its spans with
/// `metered_info_span!(that_name; ...)`. The value must be a valid Rust
/// identifier. Default behavior (opener named after the type) is unchanged.
///
/// # Runtime crate path (`crate`)
///
/// Generated code reaches the `metered-tracing` runtime (its traits and the
/// `__rt::tracing` re-export) through `::metered_tracing` by default, so the
/// deriving crate needs a **direct `metered-tracing` dependency**. Facade-only
/// consumers -- depending on `metered` with `features = ["tracing"]` and no
/// direct `metered-tracing` dependency -- override the emitted path with the
/// `#[span(crate = "::metered::tracing")]` container attribute (mirroring
/// `#[serde(crate = "...")]`). The path is spliced into the
/// `#[macro_export]`ed opener too: prefer an absolute (`::`-rooted) path; a
/// `crate::...`-rooted path is rewritten to `$crate::...` inside the opener
/// so it still resolves to the deriving crate when the opener is invoked
/// from elsewhere.
///
/// ```ignore
/// #[derive(Clone, PartialEq, Eq, Hash, metered::LabelSet, metered_tracing::SpanLabels)]
/// #[span(name = "rpc.server", help = "RPC server calls")]
/// struct RpcLabels {
///     #[span("rpc.method")]
///     rpc_method: String,
///     #[span("rpc.grpc.status_code", default = "OK", on_close)]
///     rpc_status: String, // any T: FromStr + Display + Default
/// }
///
/// let span = metered_info_span!(RpcLabels; rpc_method = "CreateOrder".to_owned());
/// RpcLabels::record_rpc_status(&span, "OK".to_owned());
/// ```
#[proc_macro_derive(SpanLabels, attributes(span))]
pub fn derive_span_labels(input: TokenStream) -> TokenStream {
    span_metric::span_labels(input)
}
