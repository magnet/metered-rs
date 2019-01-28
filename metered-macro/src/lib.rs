#![feature(proc_macro_diagnostic, proc_macro_span)]
#![feature(custom_attribute)]
// The `quote!` macro requires deep recursion.
#![recursion_limit = "512"]

extern crate proc_macro;
extern crate proc_macro2;

#[macro_use]
extern crate syn;
#[macro_use]
extern crate quote;


mod attrs_common;
mod measure_opts;
mod metered_opts;
mod metered;

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn metered(attrs: TokenStream, item: TokenStream) -> TokenStream {
    metered::metered(attrs, item)
}
