//! Procedural macros for Metered, a metric library for Rust.
//!
//! Please check the Metered crate for more documentation.

#![deny(warnings)]
// The `quote!` macro requires deep recursion.
#![recursion_limit = "512"]

extern crate proc_macro;

#[macro_use]
extern crate syn;
#[macro_use]
extern crate quote;

mod measure_opts;
mod metered;
mod metered_opts;

use proc_macro::TokenStream;

/// A procedural macro that generates a metric registry for an `impl` block.
///
/// ``` ignore
/// # extern crate metered;
/// # extern crate rand;
///
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
///
/// # fn main() {
/// # }
/// ```
///
/// ### The `metered` attribute
///
/// `#[metered(registry = YourRegistryName, registry_expr = self.wrapper.my_registry)]`
///
/// `registry` is mandatory and must be a valid Rust ident.
///
/// `registry_expr` defaults to `self.metrics`, alternate values must be a valid Rust expression.
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
/// The `type` keyword is allowed because other keywords are planned for future extra attributes (e.g, instantation options).
///
/// When `measure` attribute is applied to an `impl` block, it applies for every method that has a `measure` attribute. If a method does not need extra measure infos, it is possible to annotate it with simply `#[measure]` and the `impl` block's `measure` configuration will be applied.
///
/// The `measure` keyword can be added several times on an `impl` block or method, which will add to the list of metrics applied. Adding the same metric several time will lead in a name clash.

#[proc_macro_attribute]
pub fn metered(attrs: TokenStream, item: TokenStream) -> TokenStream {
    metered::metered(attrs, item).unwrap_or_else(|e| TokenStream::from(e.to_compile_error()))
}
