//! # Fast, ergonomic metrics for Rust!
//!
//! This crate is the one-dependency facade over the metered workspace:
//!
//! * **The core model, re-exported wholesale.** Everything in `metered-core`
//!   -- primitives, histograms, families, registries, schema/values, and the
//!   [`MetricTree`]/[`LabelSet`] derive macros -- is available here under the
//!   same flat paths. Applications depend on `metered` alone and the pieces
//!   always move together.
//! * **Satellites behind features.** The sink and telemetry crates are
//!   optional dependencies surfaced as facade modules: enable `om` for
//!   OpenMetrics text exposition (the `om` module), `tracing` for
//!   tracing-subscriber layers (the `tracing` module), and `telemetry-tokio` /
//!   `telemetry-process` / `telemetry-system` for runtime, process, and host
//!   telemetry. The `exemplar-context` feature forwards
//!   `metered-core/exemplar-context` (the ambient, thread-local exemplar
//!   context). `full` turns everything on.
//! * **Libraries can go one level down.** A library that only *produces*
//!   metrics can depend on `metered-core` directly for a maximally stable,
//!   sink-free surface; its trees compose into any application using the
//!   facade, because the facade re-exports the very same types.
//!
//! See the [`metered-core` documentation](metered_core) for the full model
//! reference; its items are documented here as re-exports.

#![deny(missing_docs)]
// NB: intentionally *not* `#![deny(warnings)]`. Denying all warnings in a
// published library means a new compiler/clippy lint can break every
// downstream build on a newer toolchain until the crate is patched -- a
// dependency-hell trap. Lint denial belongs in CI (`RUSTFLAGS="-D warnings"`).

// The whole core model: public modules, the flat re-exports, and the derive
// macros (which emit `::metered::…` paths that land right back on this glob).
pub use metered_core::*;

/// OpenMetrics text exposition: encoder, incremental renderer, parser, and the
/// optional hyper scrape endpoint (`metered-om`, behind the `om` feature).
#[cfg(feature = "om")]
pub use metered_om as om;

/// Tracing-subscriber layers exporting span telemetry as metric trees
/// (`metered-tracing`, behind the `tracing` feature).
#[cfg(feature = "tracing")]
pub use metered_tracing as tracing;

/// Tokio runtime and task telemetry as metric trees
/// (`metered-telemetry-tokio`, behind the `telemetry-tokio` feature).
#[cfg(feature = "telemetry-tokio")]
pub use metered_telemetry_tokio as telemetry_tokio;

/// Process telemetry (CPU, memory, file descriptors) as metric trees
/// (`metered-telemetry-process`, behind the `telemetry-process` feature).
#[cfg(feature = "telemetry-process")]
pub use metered_telemetry_process as telemetry_process;

/// Host system telemetry (CPU, memory, swap, load, uptime) as metric trees
/// (`metered-telemetry-system`, behind the `telemetry-system` feature).
#[cfg(feature = "telemetry-system")]
pub use metered_telemetry_system as telemetry_system;
