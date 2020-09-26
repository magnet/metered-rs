use crate::error_count_opts::ErrorCountKeyValAttribute;
use heck::SnakeCase;
use proc_macro::TokenStream;
use syn::{Ident, ItemEnum};

pub fn error_count(attrs: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let attrs: ErrorCountKeyValAttribute = syn::parse(attrs)?;
    let attrs = attrs.to_error_count_opts();
    let vis = attrs.visibility;
    let metrics_ident = attrs.name_ident;

    let input: ItemEnum = syn::parse(item)?;
    let ident = &input.ident;

    let variants = input.variants.iter().map(|v| &v.ident);
    let stringified_variants = input.variants.iter().map(|v| v.ident.to_string());
    let snake_variants: Vec<Ident> = input
        .variants
        .iter()
        .map(|v| Ident::new(&v.ident.to_string().to_snake_case(), v.ident.span()))
        .collect();

    // generate unbound arg params for each enum variant
    let variants_args = input.variants.iter().map(|v| match &v.fields {
        syn::Fields::Named(_) => quote!({ .. }),
        syn::Fields::Unnamed(v) => {
            let args = v.unnamed.iter().map(|_| quote!(_));
            quote! {
                (#( #args, )*)
            }
        }
        syn::Fields::Unit => quote!(),
    });

    Ok(quote! {
        #input

        #[derive(serde::Serialize, Default, Debug)]
        #[allow(missing_docs)]
        #vis struct #metrics_ident<C: metered::metric::Counter = metered::atomic::AtomicInt<u64>> {
            #(
                #[serde(rename = #stringified_variants, serialize_with = "metered::error_variant_serializer")]
                pub #snake_variants: C,
            )*
        }

        impl<C: metered::metric::Counter> metered::clear::Clear for #metrics_ident<C> {
            fn clear(&self) {
                #( self.#snake_variants.clear(); )*
            }
        }

        impl<T, C: metered::metric::Counter> metered::metric::Metric<Result<T, #ident>> for #metrics_ident<C> {}

        impl<C: metered::metric::Counter> metered::metric::Enter for #metrics_ident<C> {
            type E = ();
            fn enter(&self) {}
        }

        impl<T, C: metered::metric::Counter> metered::metric::OnResult<Result<T, #ident>> for #metrics_ident<C> {
            fn on_result(&self, _: (), r: &Result<T, #ident>) -> metered::metric::Advice {
                if let Err(e) = r {
                    match e {
                        #( #ident::#variants #variants_args => self.#snake_variants.incr(), )*
                    }
                }
                metered::metric::Advice::Return
            }
        }
    }.into())
}
