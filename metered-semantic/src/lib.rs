//! # Method-instrumentation model for Metered
//!
//! `metered-semantic` is the method-level, semantic instrumentation model that
//! historically shipped inside `metered`. It builds on the OpenMetrics-native
//! `metered` core (counters, gauges, histograms, metric trees, registries and
//! schema) and adds the ergonomic measuring layer on top.
//!
//! It provides:
//!
//! * **Semantic measuring wrappers** -- [`HitCount`], [`ErrorCount`],
//!   [`NoneCount`], [`InFlight`] and [`Elapsed`] -- thin wrappers over the core
//!   primitives that know how to record an *expression's* outcome (including on
//!   panic, early exit, or async cancellation).
//! * **The `measure!` macro** and the [`Measure`]/[`Recorder`] traits that power
//!   it: enter a metric, run an expression, record the outcome exactly once.
//! * **The `#[metered]` and `#[error_count]` procedural macros** that generate a
//!   metric registry for an `impl` block and a per-variant error breakdown for
//!   an error enum.
//! * **Explicit recording** ([`recording`], behind the `recording` feature) for
//!   code that does not use tracing spans.
//! * **A migration summary view** ([`migration`], behind the `migration`
//!   feature) that exposes a legacy summary shape derived from a native
//!   histogram so dashboards can migrate without a flag day.
//!
//! The generated code refers to `::metered_semantic::` absolute paths, so it is
//! immune to local shadowing. The core OpenMetrics model -- and its sinks such
//! as `metered-om` -- live in the `metered` crate; this crate only adds the
//! semantic measuring layer.
//!
//! ## Example using procedural macros
//!
//! ```rust,ignore
//! use metered_semantic::{metered, Elapsed, HitCount};
//!
//! #[derive(Default, Debug)]
//! pub struct Biz {
//!     metrics: BizMetrics,
//! }
//!
//! #[metered_semantic::metered(registry = BizMetrics)]
//! impl Biz {
//!     #[measure([HitCount, Elapsed])]
//!     pub fn biz(&self) {
//!         let delay = std::time::Duration::from_millis(rand::random::<u64>() % 200);
//!         std::thread::sleep(delay);
//!     }
//! }
//! ```
//!
//! ## Example using `measure!` directly
//!
//! ```rust,ignore
//! use metered_semantic::{measure, HitCount, ErrorCount};
//!
//! #[derive(Default, Debug)]
//! struct TestMetrics {
//!     hit_count: HitCount,
//!     error_count: ErrorCount,
//! }
//!
//! fn test(should_fail: bool, metrics: &TestMetrics) -> Result<u32, &'static str> {
//!     let hit_count = &metrics.hit_count;
//!     let error_count = &metrics.error_count;
//!     measure!(hit_count, {
//!         measure!(error_count, {
//!             if should_fail {
//!                 Err("Failed!")
//!             } else {
//!                 Ok(42)
//!             }
//!         })
//!     })
//! }
//! ```

#![deny(missing_docs)]

// Core OpenMetrics symbols referenced by the macros and the measuring wrappers,
// re-exported so generated code can use `::metered_semantic::` paths uniformly.
pub use metered::handle;
pub use metered::{
    join_name, Counter, CounterSource, MetricSchema, MetricTree, MetricType, MetricValues,
};

pub mod metric;

mod common;
mod metric_impls;
pub use common::{Elapsed, ElapsedConfig, ErrorCount, HitCount, InFlight, NoneCount};

mod error_breakdown;
pub use error_breakdown::{
    ClassifyError, ErrorBreakdown, ErrorBreakdownIncr, ErrorBreakdownRecorder,
};

#[cfg(feature = "recording")]
pub mod recording;

#[cfg(feature = "migration")]
pub mod migration;

pub use metric::{Measure, Recorder};

pub use metered_macro::{error_count, metered};

/// The `measure!` macro takes a reference to a metric and an expression.
///
/// It applies the metric and the expression is returned unchanged.
///
/// ```rust,ignore
/// use metered_semantic::{Elapsed, measure};
///
/// let elapsed: Elapsed = Elapsed::default();
///
/// measure!(&elapsed, {
///     std::thread::sleep(std::time::Duration::from_millis(1));
/// });
///
/// assert_eq!(elapsed.snapshot().count, 1);
/// ```
///
/// It also allows to pass an array of references, which will expand recursively.
///
/// ```rust,ignore
/// use metered_semantic::{HitCount, Elapsed, measure};
///
/// let hit_count: HitCount = HitCount::default();
/// let elapsed: Elapsed = Elapsed::default();
///
/// measure!([&hit_count, &elapsed], {
///     std::thread::sleep(std::time::Duration::from_millis(1));
/// });
///
/// assert_eq!(hit_count.get(), 1);
/// assert_eq!(elapsed.snapshot().count, 1);
/// ```
///
#[macro_export]
macro_rules! measure {
    ([$metric:expr], $expr:expr) => {{
        $crate::measure!($metric, $expr)
    }};

    ([$metric:expr, $($metrics:expr),*], $expr:expr) => {
        $crate::measure!($metric, $crate::measure!([$($metrics),*], $expr))
    };

    ($metric:expr, $e:expr) => {{
        // Enter the metric, obtaining an owned recorder that holds its own
        // handle (it does not borrow the metric or `self`). The recorder is
        // held across `$e` -- so `$e` may take `&mut self` and `.await` -- and
        // records the outcome exactly once: `complete` on normal completion, or
        // the recorder's `Drop` on panic / early exit / async cancellation.
        let mut __metered_recorder = $crate::metric::Measure::enter($metric);
        let __metered_result = $e;
        $crate::metric::Recorder::complete(&mut __metered_recorder, &__metered_result);
        __metered_result
    }};
}
