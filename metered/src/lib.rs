//! # Fast, ergonomic metrics for Rust!
//!
//! Metered helps you measure the performance of your programs in production.
//! Inspired by Coda Hale's Java metrics library, Metered makes live
//! measurements easy by providing measurement declarative and procedural
//! macros, and a variety of useful metrics ready out-of-the-box:
//! * [`HitCount`]: a counter tracking how much a piece of code was hit.
//! * [`ErrorCount`]: a counter tracking how many errors were returned -- (works
//!   on any expression returning a std `Result`)
//! * [`InFlight`]: a gauge tracking how many requests are active
//! * [`ResponseTime`]: statistics backed by an HdrHistogram of the duration of
//!   an expression
//! * [`Throughput`]: statistics backed by an HdrHistogram of how many times an
//!   expression is called per second.
//!
//! These metrics are usually applied to methods, using provided procedural
//! macros that generate the boilerplate.
//!
//! To achieve higher performance, these stock metrics can be customized to use
//! non-thread safe (`!Sync`/`!Send`) datastructures, but they default to
//! thread-safe datastructures implemented using lock-free strategies where
//! possible. This is an ergonomical choice to provide defaults that work in all
//! situations.
//!
//! Metered is designed as a zero-overhead abstraction -- in the sense that the
//! higher-level ergonomics should not cost over manually adding metrics.
//! Notably, stock metrics will *not* allocate memory after they're initialized
//! the first time.  However, they are triggered at every method call and it can
//! be interesting to use lighter metrics (e.g
//! [`HitCount`]) in hot code paths and favour
//! heavier metrics ([`Throughput`],
//! [`ResponseTime`]) in higher-level entry
//! points.
//!
//! If a metric you need is missing, or if you want to customize a metric (for
//! instance, to track how many times a specific error occurs, or react
//! depending on your return type), it is possible to implement your own metrics
//! simply by implementing the [`Metric`] trait .
//!
//! Metered does not use statics or shared global state. Instead, it lets you
//! either build your own metric registry using the metrics you need, or can
//! generate a metric registry for you using method attributes. Metered will
//! generate one registry per `impl` block annotated with the `metered`
//! attribute, under the name provided as the `registry` parameter. By default,
//! Metered will expect the registry to be accessed as `self.metrics` but the
//! expression can be overridden with the `registry_expr` attribute parameter.
//! See the demos for more examples.
//!
//! Metered will generate metric registries that derive [`std::fmt::Debug`] and
//! [`serde::Serialize`] to extract your metrics easily. Metered generates one
//! sub-registry per method annotated with the `measure` attribute, hence
//! organizing metrics hierarchically. This ensures access time to metrics in
//! generated registries is always constant (and, when possible,
//! cache-friendly), without any overhead other than the metric itself.
//!
//! Metered will happily measure any method, whether it is `async` or not, and
//! the metrics will work as expected (e.g,
//! [`ResponseTime`] will return the completion
//! time across `await`'ed invocations).
//!
//! Metered's serialized metrics can be used in conjunction with
//! [`serde_prometheus`](https://github.com/w4/serde_prometheus) to publish
//! metrics to Prometheus.
//!
//! ## Example using procedural macros (recommended)
//!
//! ```
//! # extern crate metered;
//! # extern crate rand;
//!
//! use metered::{metered, Throughput, HitCount};
//!
//! #[derive(Default, Debug)]
//! pub struct Biz {
//!     metrics: BizMetrics,
//! }
//!
//! #[metered::metered(registry = BizMetrics)]
//! impl Biz {
//!     #[measure([HitCount, Throughput])]
//!     pub fn biz(&self) {
//!         let delay = std::time::Duration::from_millis(rand::random::<u64>() % 200);
//!         std::thread::sleep(delay);
//!     }
//! }
//!
//! # fn main() {
//! # }
//! ```
//!
//! In the snippet above, we will measure the
//! [`HitCount`] and
//! [`Throughput`] of the `biz` method.
//!
//! This works by first annotating the `impl` block with the `metered`
//! annotation and specifying the name Metered should give to the metric
//! registry (here `BizMetrics`). Later, Metered will assume the expression to
//! access that repository is `self.metrics`, hence we need a `metrics` field
//! with the `BizMetrics` type in `Biz`. It would be possible to use another
//! field name by specificying another registry expression, such as
//! `#[metered(registry = BizMetrics, registry_expr = self.my_custom_metrics)]`.
//!
//! Then, we must annotate which methods we wish to measure using the `measure`
//! attribute, specifying the metrics we wish to apply: the metrics here are
//! simply types of structures implementing the `Metric` trait, and you can
//! define your own. Since there is no magic, we must ensure `self.metrics` can
//! be accessed, and this will only work on methods with a `&self` or `&mut
//! self` receiver.
//!
//! ## Example of manually using metrics
//!
//! ```
//! use metered::{measure, HitCount, ErrorCount};
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
//!
//! The code above shows how different metrics compose, and in general the kind
//! of boilerplate generated by the `#[metered]` procedural macro.

#![deny(missing_docs)]
// #![deny(warnings)]

pub mod atomic;
pub mod clear;
pub mod common;
pub mod hdr_histogram;
pub mod int_counter;
pub mod int_gauge;
pub mod metric;
pub(crate) mod num_wrapper;
pub mod time_source;

pub use common::{ErrorCount, HitCount, InFlight, ResponseTime, Throughput};
pub use metered_macro::{error_count, metered};
pub use metric::{Counter, Gauge, Histogram, Metric};

/// Re-export this type so 3rd-party crates don't need to depend on the
/// `aspect-rs` crate.
pub use aspect::Enter;

/// The `measure!` macro takes a reference to a metric and an expression.
///
/// It applies the metric and the expression is returned unchanged.
///
/// ```rust
/// use metered::{ResponseTime, measure};
///
/// let response_time: ResponseTime = ResponseTime::default();
///
/// measure!(&response_time, {
///     std::thread::sleep(std::time::Duration::from_millis(100));
/// });
///
/// assert!(response_time.histogram().mean() > 0.0);
/// ```
///
/// It also allows to pass an array of references, which will expand
/// recursively.
///
/// ```rust
/// use metered::{HitCount, ResponseTime, measure};
///
/// let hit_count: HitCount = HitCount::default();
/// let response_time: ResponseTime = ResponseTime::default();
///
/// measure!([&hit_count, &response_time], {
///     std::thread::sleep(std::time::Duration::from_millis(100));
/// });
///
/// assert_eq!(hit_count.get(), 1);
/// assert!(response_time.histogram().mean() > 0.0);
/// ```
#[macro_export]
macro_rules! measure {
    ([$metric:expr], $expr:expr) => {{
        $crate::measure!($metric, $expr)
    }};

    ([$metric:expr, $($metrics:expr),*], $expr:expr) => {
        $crate::measure!($metric, $crate::measure!([$($metrics),*], $expr))
    };

    ($metric:expr, $e:expr) => {{
        let metric = $metric;
        let guard = $crate::metric::ExitGuard::new(metric);
        let mut result = $e;
        guard.on_result(&mut result);
        result
    }};
}

/// Serializer for values within a struct generated by
/// `metered::metered_error_variants` that adds an `error_kind` label when being
/// serialized by `serde_prometheus`.
pub fn error_variant_serializer<S: serde::Serializer, T: serde::Serialize>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_newtype_struct("!|variant[::]==<", value)
}

/// Serializer for values within a struct generated by
/// `metered::metered_error_variants` that adds an `error_kind` label when being
/// serialized by `serde_prometheus`. If the `value` has been cleared. This
/// operation is a no-op and the value wont be written to the `serializer`.
pub fn error_variant_serializer_skip_cleared<
    S: serde::Serializer,
    T: serde::Serialize + clear::Clearable,
>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    if value.is_cleared() {
        serializer.serialize_none()
    } else {
        error_variant_serializer(value, serializer)
    }
}

/// Trait applied to error enums by `#[metered::error_count]` to identify
/// generated error count structs.
pub trait ErrorBreakdown<C: metric::Counter> {
    /// The generated error count struct.
    type ErrorCount;
}

/// Generic trait for `ErrorBreakdown::ErrorCount` to increase error count for a
/// specific variant by 1.
pub trait ErrorBreakdownIncr<E> {
    /// Increase count for given variant by 1.
    fn incr(&self, e: &E);
}
