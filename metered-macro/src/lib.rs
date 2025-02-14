//! Procedural macros for Metered, a metric library for Rust.
//!
//! Please check the Metered crate for more documentation.

// #![deny(warnings)]
// The `quote!` macro requires deep recursion.
#![recursion_limit = "512"]

#[macro_use]
extern crate syn;
#[macro_use]
extern crate quote;

mod error_count;
mod error_count_opts;
mod measure_opts;
mod metered;
mod metered_opts;

use proc_macro::TokenStream;

/// A procedural macro that generates a metric registry for an `impl` block.
///
/// ```
/// use metered::{metered, Throughput, HitCount};
///
/// #[derive(Default, Debug)]
/// pub struct Biz {
///     metrics: BizMetrics,
/// }
///
/// #[metered::metered(registry = BizMetrics)]
/// impl Biz {
///     #[measure([HitCount, Throughput])]
///     pub fn biz(&self) {
///         let delay = std::time::Duration::from_millis(rand::random::<u64>() % 200);
///         std::thread::sleep(delay);
///     }
/// }
/// #
/// # let biz = Biz::default();
/// # biz.biz();
/// # assert_eq!(biz.metrics.biz.hit_count.0.get(), 1);
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

#[proc_macro_attribute]
pub fn metered(attrs: TokenStream, item: TokenStream) -> TokenStream {
    metered::metered(attrs, item).unwrap_or_else(|e| TokenStream::from(e.to_compile_error()))
}

/// A procedural macro that generates a new metric that measures the amount
/// of times each variant of an error has been thrown, to be used as
/// crate-specific replacement for `metered::ErrorCount`.
///
/// ```
/// # use metered_macro::{metered, error_count};
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
/// - `skip_cleared` allows to make the serializer skip "cleared" entries, that
///   is entries for which the `Clearable::is_cleared` function returns true
///   (for counters, by default, whether they are 0). It defaults to whether the
///   feature `error-count-skip-cleared-by-default` is enabled. By default, this
///   feature is disabled, and no entry will be skipped.
///
///
/// The `error_count` macro may only be applied to any enums that have a
/// `std::error::Error` impl. The generated struct may then be included
/// in `measure` attributes to measure the amount of errors returned of
/// each variant defined in your error enum.

#[proc_macro_attribute]
pub fn error_count(attrs: TokenStream, item: TokenStream) -> TokenStream {
    error_count::error_count(attrs, item)
        .unwrap_or_else(|e| TokenStream::from(e.to_compile_error()))
}
