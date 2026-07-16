//! Tracing subscriber layers that export span telemetry as `metered` metrics.
//!
//! This crate deliberately depends on the public `tracing` ecosystem, not on a
//! service framework. Service identity (`service.name`, version, environment,
//! ...) should be modeled by the service container and exported separately as a
//! `metered::Info` value or as shared registry labels.
//!
//! # Semantic span metrics
//!
//! Spans are not folded into a single generic `span_*` family. Instead you
//! declare one [`SpanMetric`] per *semantic* span (RPC server, DB client,
//! background worker, ...). Each one:
//!
//! - matches spans by name (e.g. `rpc.server`),
//! - on close, records a `<name>_requests_total` counter and a
//!   `<name>_duration_seconds` histogram,
//! - labels both from the span's semantic fields (e.g. `rpc.method`), with
//!   per-label defaults when the field is absent.
//!
//! A [`SpanMetric`] is a cheap clonable handle: hand one clone to the
//! [`TracingMetrics`] layer (so span closes are recorded) and place another in
//! your [`metered`] metric view under a semantic name (so it is exported where
//! it belongs).
//!
//! ```
//! use metered_tracing::{SpanMetric, TracingMetrics};
//! use metered::Registry;
//! use metered_om::OpenMetricsRegistryExt;
//! use tracing_subscriber::prelude::*;
//!
//! let rpc = SpanMetric::for_span("rpc.server")
//!     .help("RPC server calls")
//!     .label("rpc_method", "rpc.method")
//!     .build();
//!
//! let layer = TracingMetrics::builder().recorder(rpc.clone()).build();
//! let subscriber = tracing_subscriber::registry().with(layer);
//! tracing::subscriber::with_default(subscriber, || {
//!     let span = tracing::info_span!("rpc.server", rpc.method = "CreateOrder");
//!     let _entered = span.enter();
//! });
//!
//! let mut registry = Registry::new();
//! registry.register(metered::entry::metric("rpc_server").source(&rpc));
//! let text = registry.encode_to_string().unwrap();
//! assert!(text.contains("rpc_server_requests_total{rpc_method=\"CreateOrder\"} 1"));
//! ```

mod dispatch;
mod exemplar;
mod fields;
mod layer;
mod recorder;
mod span_metric;

pub use dispatch::{MalformedSpans, SpanMetricsSource, TracingMetrics, TracingMetricsBuilder};
pub use exemplar::{ExemplarProvider, NoExemplarProvider};
#[cfg(feature = "exemplar")]
pub use exemplar::{FieldExemplarProvider, TracingExemplarLayer};
pub use fields::{FieldValue, FieldValueError, FromFieldValue, SpanFields};
pub use recorder::{FromSpanFields, SpanDurations, SpanFieldsError, SpanRecorder};
pub use span_metric::{SpanMetric, SpanMetricBuilder, DEFAULT_LABEL_VALUE_CAP};

#[cfg(feature = "exemplar")]
pub(crate) use span_metric::sanitize_ident;

/// Derives the span/metric wiring for a typed labels struct.
///
/// You declare the labels as a normal `#[derive(LabelSet)]` struct -- the field
/// names are the OpenMetrics labels, their types are enforced -- and add
/// `#[span("otel.field")]` to map each to the OpenTelemetry semconv span field it
/// reads from. `SpanLabels` then generates:
///
/// - [`FromSpanFields`] -- builds the typed key from a closed span (parsing each
///   value with `FromStr`), so a [`SpanDurations`] adapter keys its histogram by
///   the labels the span carries;
/// - the call-site span opener reached through [`metered_info_span!`], so the
///   emitted span uses the same field names (no drift);
/// - typed `record_*` setters for `on_close` fields, and `SPAN` / `HELP` consts.
///
/// The metrics themselves are *normal* `metered` families you own: a counter you
/// bump directly, and a dynamic-exponential-histogram family the layer times from
/// the span through a stateless [`SpanDurations`] adapter. The labels struct is
/// constructed every observation, so there is no dead code.
///
/// ```
/// use metered::{entry::metric, Counter, DynamicExponentialHistogram, Family, LabelSet, Registry};
/// use metered_om::OpenMetricsRegistryExt;
/// use metered_tracing::{metered_info_span, SpanDurations, SpanLabels};
/// use std::sync::atomic::AtomicU64;
/// use std::sync::Arc;
/// use tracing_subscriber::prelude::*;
///
/// #[derive(Clone, PartialEq, Eq, Hash, LabelSet, SpanLabels)]
/// #[span(name = "rpc.server", help = "RPC server calls")]
/// struct RpcLabels {
///     #[span("rpc.method")]
///     rpc_method: String,
///     #[span("rpc.grpc.status_code", default = "OK", on_close)]
///     rpc_status: String, // any T: FromStr + Display + Default
/// }
///
/// // A counter you own and bump directly...
/// let requests: Family<RpcLabels, AtomicU64> = Family::default();
/// // ...and a component that owns the duration histogram the layer times into.
/// // The `Arc` wraps the *component*, not the metric; the adapter projects to it.
/// struct Rpc {
///     duration: Family<RpcLabels, DynamicExponentialHistogram>,
/// }
/// let rpc = Arc::new(Rpc { duration: Family::default() });
///
/// let layer = metered_tracing::TracingMetrics::builder()
///     .recorder(SpanDurations::on(RpcLabels::SPAN, &rpc, |rpc: &Rpc| &rpc.duration))
///     .build();
/// let subscriber = tracing_subscriber::registry().with(layer);
///
/// tracing::subscriber::with_default(subscriber, || {
///     let span = metered_info_span!(RpcLabels; rpc_method = "CreateOrder".to_owned());
///     span.in_scope(|| {});
///     RpcLabels::record_rpc_status(&span, "OK".to_owned()); // typed, on_close
///     // The middleware bumps the counter directly, keyed by the same labels.
///     let labels = RpcLabels { rpc_method: "CreateOrder".to_owned(), rpc_status: "OK".to_owned() };
///     requests.with(&labels, Counter::incr);
/// });
///
/// let mut registry = Registry::new();
/// registry.register(metric("rpc_server_requests").source(&requests));
/// registry.register(metric("rpc_server_duration_seconds").source(&rpc.duration).unit("seconds"));
/// let text = registry.encode_to_string().unwrap();
/// assert!(text.contains("rpc_server_requests_total{rpc_method=\"CreateOrder\",rpc_status=\"OK\"} 1"));
/// assert!(text.contains("rpc_server_duration_seconds_count{rpc_method=\"CreateOrder\",rpc_status=\"OK\"} 1"));
/// ```
pub use metered_macro::SpanLabels;

/// `tracing` (and any other runtime deps) reached by `metered-macro`-generated
/// code, so a crate that derives [`SpanLabels`] needs no direct `tracing`
/// dependency. Not a stable API: do not use these paths directly.
#[doc(hidden)]
pub mod __rt {
    pub use ::tracing;
}

/// Opens the span for a [`SpanLabels`]-derived labels type, keyed by the type:
/// `metered_info_span!(RpcLabels; rpc_method = m, "trace_id" = id)`.
///
/// Eager label fields are passed by name (their declared types are enforced);
/// any trailing `"field" = value` pairs are extra span-only context. `on_close`
/// fields start `Empty` and are set with the generated `record_*` functions.
///
/// This dispatches to the opener the derive `#[macro_export]`s under the labels
/// type's name, so it works from any module and before the type's textual
/// definition -- not only where the struct is declared.
#[macro_export]
macro_rules! metered_info_span {
    ($labels:ident; $($field:tt)*) => {
        $labels!(@open $($field)*)
    };
}
