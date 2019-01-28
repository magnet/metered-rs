// The `quote!` macro requires deep recursion.
#![recursion_limit = "512"]

extern crate proc_macro;

#[macro_use]
extern crate syn;
#[macro_use]
extern crate quote;

mod attrs_common;
mod measure_opts;
mod metered;
mod metered_opts;

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn metered(attrs: TokenStream, item: TokenStream) -> TokenStream {
    metered::metered(attrs, item)
}
