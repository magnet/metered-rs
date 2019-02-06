//! # Fast, ergonomic metrics for Rust!
//!
//! Metered helps you measure the performance of your programs in production. Inspired by Coda Hale's Java metrics library, Metered makes live measurements easy by providing measurement declarative and procedural macros, and a variety of useful metrics ready out-of-the-box:
//! * [`HitCount`](common/struct.HitCount.html): a counter tracking how much a piece of code was hit.
//! * [`ErrorCount`](common/struct.ErrorCount.html): a counter tracking how many errors were returned -- (works on any expression returning a std `Result`)
//! * [`InFlight`](common/struct.InFlight.html): a gauge tracking how many requests are active
//! * [`ResponseTime`](common/struct.ResponseTime.html): statistics backed by an HdrHistogram of the duration of an expression
//! * [`Throughput`](common/struct.Throughput.html): statistics backed by an HdrHistogram of how many times an expression is called per second.
//!
//! These metrics are usually applied to methods, with provided procedural macros that generate the boilerplate for us.
//!
//! For better performance, these stock metrics can be customized to use non-thread safe (`!Sync`/`!Send`) datastructures. For ergonomy reasons, stock metrics default to thread-safe datastructures, implemented using lock-free strategies where possible.
//!
//! Metered is designed as a zero-overhead abstraction -- in the sense that the higher-level ergonomics should not cost over manually adding metrics. Stock metrics will *not* allocate memory after they're initialized the first time.  However, they are triggered at every method call and it can be interesting to use lighter metrics (e.g [`HitCount`](common/struct.HitCount.html)) in very hot code paths and favour heavier metrics ([`Throughput`](common/struct.Throughput.html), [`ResponseTime`](common/struct.ResponseTime.html)) in entry points.
//!
//! If a metric you need is missing, or if you want to customize a metric (for instance, to track how many times a specific error occurs, or react depending on your return type), it is possible to implement your own metrics simply by implementing the [`Metric`](metric/trait.Metric.html) trait .
//!
//! Metered does not use statics or shared global state. Instead, it lets you either build your own metric registry using the metrics you need, or can generate a metric registry for you using method attributes. Metered will generate one registry per `impl` block annotated with the `metered` attribute, under the name provided as the `registry` parameter. By default, Metered will expect the registry to be accessed as `self.metrics` but the expression can be overridden with the `registry_expr` attribute parameter. See the demo for more examples.
//!
//! Metered will generate metric registries that derive `Debug` and `serde::Serialize` to extract your metrics easily. Adapters for metric storage and monitoring systems are planned (contributions welcome!). Metered generates one sub-registry per method annotated with the `measure` attribute, hence organizing metrics hierarchically. This ensures access time to metrics in generated registries is always constant (and, when possible, cache-friendly), without any overhead other than the metric itself.
//!
//! Metered will happily measure any method, whether it is `async` or not, and the metrics will work as expected (e.g, [`ResponseTime`](common/struct.ResponseTime.html) will return the completion time across `await!` invocations).
//!
//! ## Example using procedural macros (recommended)
//!
//! ```rust
//! use metered::{metered, Throughput, HitCount};
//!
//! #[derive(Default, Debug, serde::Serialize)]
//! pub struct Biz {
//!     metrics: BizMetrics,
//! }
//!
//! #[metered(registry = BizMetrics)]
//! impl Biz {
//!     #[measure([HitCount, Throughput])]
//!     pub fn biz(&self) {        
//!         let delay = std::time::Duration::from_millis(rand::random::<u64>() % 200);
//!         std::thread::sleep(delay);
//!     }   
//! }
//! ```
//!
//! In the snippet above, we will measure the [`HitCount`](common/struct.HitCount.html) and [`Throughput`](common/struct.Throughput.html) of the `biz` method.
//!
//! This works by first annotating the `impl` block with the `metered` annotation and specifying the name Metered should give to the metric registry (here `BizMetrics`). Later, Metered will assume the expression to access that repository is `self.metrics`, hence we need a `metrics` field with the `BizMetrics` type in `Biz`. It would be possible to use another field name by specificying another registry expression, such as `#[metered(registry = BizMetrics, registry_expr = self.my_custom_metrics)]`.
//!
//! Then, we must annotate which methods we wish to measure using the `measure` attribute, specifying the metrics we wish to apply: the metrics here are simply types of structures implementing the `Metric` trait, and you can define your own. Since there is no magic, we must ensure `self.metrics` can be accessed, and this will only work on methods with a `&self` or `&mut self` receiver.
//!
//! ## Example of manually using metrics
//!
//! ```rust
//! #[derive(Default, Debug, serde::Serialize)]
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
//! The code above shows how different metrics compose, and in general the kind of boilerplate generated by the `#[metered]` procedural macro.

pub mod atomic;
pub mod clear;
pub mod common;
pub mod hdr_histogram;
pub mod int_counter;
pub mod int_gauge;
pub mod metric;
pub mod time_source;

pub use common::{ErrorCount, HitCount, InFlight, ResponseTime, Throughput};
pub use metered_macro::metered;

/// Re-export this type so 3rd-party crates don't need to depend on the `aspect-rs` crate.
pub use aspect::{Advice, Enter};

/// The `measure!` macro takes a reference to a metric and an expression.
///
/// It applies the metric and the expression is returned unchanged.
aspect::define!(measure: metered::metric::on_result);
