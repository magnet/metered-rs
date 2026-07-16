//! # The metered core model
//!
//! This crate is the core metric model of the metered workspace: metric state,
//! composition, schema/value collection, and the derive macros. Applications
//! consume it through the `metered` facade crate, which re-exports this crate
//! wholesale and adds the optional sink and telemetry satellites; libraries
//! that only *produce* metrics can depend on `metered-core` directly for a
//! minimal, sink-free surface.
//!
//! Metered helps you measure the performance of your programs in production.
//! Inspired by Coda Hale's Java metrics library, Metered makes live
//! measurements easy by providing direct OpenMetrics primitives and metric-tree
//! registries.
//!
//! The core OpenMetrics constructs -- [`Counter`], [`Gauge`] and
//! [`BucketHistogram`] -- are exposed directly as readable, source-of-truth
//! values. The [`Histogram`] trait abstracts over the bucket and exponential
//! backends.
//!
//! The stock metrics are backed by lock-free atomics, so they are cheap to
//! update, safe to share across threads, and never allocate after
//! initialization.
//!
//! Metered aims to keep instrumentation overhead explicit and small. Stock
//! metrics allocate their backing state at initialization; the hot path then
//! records through atomics and, for metrics that must finish on completion or
//! abort, a cheap internal handle clone. Use lighter metrics such as
//! counters in the hottest paths and reserve richer histograms for entry points
//! where duration buckets are worth the extra bookkeeping.
//!
//! If a metric you need is missing, wrap the core primitives in a newtype or
//! implement [`Metric`] / [`MetricTree`] directly.
//!
//! Metered does not use statics or shared global state. Instead, it lets you
//! build your own metric registry using the metrics you need. A [`Registry`]
//! (and any [`MetricTree`]) implements [`std::fmt::Debug`] and contributes
//! schema and values through the same core model as handwritten trees. A sink crate
//! such as `metered-om` renders that schema/value pair as OpenMetrics text.
//!
//! To publish to Prometheus, encode a [`Registry`] (or any [`MetricTree`]) with
//! a sink such as `metered-om` and expose that text over an HTTP endpoint.
//!
//! ## Stability & dependency policy
//!
//! Metered is designed so it -- or parts of it -- can be upgraded without
//! dragging a whole workspace along:
//! * **OpenMetrics-native core.** The public API is the OpenMetrics model
//!   (primitives, histograms, families, registries, schema); higher-level
//!   instrumentation layers (such as `metered-tracing`) build on this core.
//! * **One direct dependency.** The derive macros are re-exported from this
//!   crate, so downstream crates depend on the `metered` facade (or on this
//!   crate) alone -- never on `metered-macro` -- and the two always move
//!   together.
//! * **Hygienic, relocatable macros.** Generated code refers to `::metered::`
//!   absolute paths, so it is immune to local shadowing.
//! * **Evolvable surface.** Open enums such as [`MetricType`] are
//!   `#[non_exhaustive]`, so new OpenMetrics constructs can be added without a
//!   breaking change.

#![deny(missing_docs)]
// NB: intentionally *not* `#![deny(warnings)]`. Denying all warnings in a
// published library means a new compiler/clippy lint can break every
// downstream build on a newer toolchain until the crate is patched -- a
// dependency-hell trap. Lint denial belongs in CI (`RUSTFLAGS="-D warnings"`).

// The source is grouped into concern clusters (`model`, `instruments`,
// `labels`), which are private: every public module inside them is re-exported
// at the crate root below, so the crate's public module paths stay flat and
// unchanged. (rustfmt keeps each blank-line-separated group sorted.)
mod instruments;
mod labels;
mod model;

// Registry & views: assemble a tree from borrowed application state.
pub mod registry;

// Exposition: render a tree to a sink or a query, or adapt application state.
pub mod adapter;
pub mod query;
pub mod sink;

// ---- Public module paths, re-exported from the concern clusters. ----

// Core model: the metric-tree traits, metadata, the describe()/collect() pair,
// and tree shaping. `handle` holds the shared backing state and is `pub`
// (doc-hidden) only because generated code refers to it.
#[doc(hidden)]
pub use model::handle;
pub use model::{meta, metric_tree, schema, shape, values};

// Instruments: the OpenMetrics metric types and the `Histogram` trait over them.
pub use instruments::{
    bucket_histogram, exponential_histogram, gauge_histogram, histogram, primitives, summary,
};

// Composition: labelled families and cardinality control.
pub use labels::{family, interner};

// ---- Public API: re-exported in the same layers as the modules above.
// (`doc(no_inline)` keeps each item documented under its module page, as it
// was when the modules were declared at the crate root.) ----

// Core model.
#[doc(no_inline)]
pub use meta::{Help, LabelName, Name, Scalar, Unit};
#[doc(no_inline)]
pub use metric_tree::{Metric, MetricTree, MetricTreeExt, MetricTreeMeta, MetricType, join_name};
#[doc(no_inline)]
pub use schema::{
    HistogramRender, MetricFamilySchema, MetricSchema, SchemaError, name_has_unit_suffix,
};
#[doc(no_inline)]
pub use values::{
    HistogramData, HistogramValue, MetricExemplar, MetricSample, MetricSampleValue, MetricValues,
};

// Instruments.
#[doc(no_inline)]
pub use bucket_histogram::{
    Bucket, BucketHistogram, Buckets, Exemplar, ExemplarSource, HistogramSnapshot, NoExemplars,
};
#[doc(no_inline)]
pub use exponential_histogram::{
    DynamicExponentialHistogram, ExponentialSnapshot, FixedExponentialHistogram,
    FixedHistogramError,
};
#[doc(no_inline)]
pub use gauge_histogram::{
    GaugeBucket, GaugeBuckets, GaugeHistogram, GaugeHistogramSnapshot, GaugeHistogramSource,
};
#[doc(no_inline)]
pub use histogram::{ExponentialHistogram, Histogram};
#[doc(no_inline)]
pub use primitives::{
    AsCounter, AsGauge, AsInfo, Counter, CounterSource, Gauge, GaugeSource, Info, InfoMetric,
    Labels, StateSet,
};
#[doc(no_inline)]
pub use summary::{QuantileSource, Summary, SummaryReading};

// Composition.
// `compose_labels` lives in a private helper module, so it is documented here
// at the crate root (no `doc(no_inline)`): the one inner-wins label composer
// that macro expansions and sinks reach as `::metered::compose_labels`.
pub use labels::slices::compose_labels;

#[doc(no_inline)]
pub use family::{Family, LabelSet};
#[doc(no_inline)]
pub use interner::BoundedValues;
#[doc(no_inline)]
pub use shape::{Flatten, Renamed};

// Registry & views.
pub use registry::entry::{Emitter, FamilyEmitter};
pub use registry::{MetricTreeView, MetricsView, Registry, entry};

// Exposition.
pub use query::{Query, QueryDialect};
pub use sink::{MetricSink, SinkError};

// Derive macros sharing names with the traits they implement (like serde).
/// The derive macros emit `::metered::…` absolute paths by default, so they
/// only compile where a `metered` crate resolves to this model. Use them
/// through the `metered` facade, from a crate depending on `metered-core`
/// directly with a renamed dependency
/// (`metered = { package = "metered-core", version = "…" }`), or override the
/// emitted path with container-level `#[metrics(crate = "...")]` on
/// `#[derive(MetricTree)]` / `#[derive(LabelSet)]` (mirroring
/// `#[serde(crate = "...")]`).
pub use metered_macro::{LabelSet, MetricTree};
