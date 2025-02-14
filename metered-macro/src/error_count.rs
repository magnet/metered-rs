use crate::error_count_opts::ErrorCountKeyValAttribute;
use heck::ToSnakeCase;
use proc_macro::TokenStream;
use syn::{spanned::Spanned, Attribute, Field, Fields, Ident, ItemEnum};

pub fn error_count(attrs: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let attrs: ErrorCountKeyValAttribute = syn::parse(attrs)?;
    let attrs = attrs.to_error_count_opts();
    let vis = attrs.visibility;
    let metrics_ident = attrs.name_ident;

    let mut input: ItemEnum = syn::parse(item)?;

    let nested_attrs = get_nested_attrs(&mut input)?;

    // get the type of the metric for each variant, most of the time this will be
    // `C`, but if `#[nested(Abc)]` is on a variant field, the type will instead
    // be set to `Abc` and incrs will be delegated there
    let metric_type = nested_attrs
        .iter()
        .map(|(_, v)| {
            if let Some((field, attr)) = v {
                let error_type = &field.ty;
                attr.parse_args::<proc_macro2::TokenStream>()
                    .unwrap_or_else(
                        |_| quote!(<#error_type as metered::ErrorBreakdown<C>>::ErrorCount),
                    )
            } else {
                quote!(C)
            }
        })
        .collect::<Vec<_>>();

    let ident = &input.ident;

    let variants = input.variants.iter().map(|v| &v.ident);
    let stringified_variants = input.variants.iter().map(|v| v.ident.to_string());
    let snake_variants: Vec<Ident> = input
        .variants
        .iter()
        .map(|v| Ident::new(&v.ident.to_string().to_snake_case(), v.ident.span()))
        .collect();

    // copy #[cfg(..)] attributes from the variant and apply them to the
    // corresponding error in our struct so we don't point to an invalid variant
    // in certain configurations.
    let cfg_attrs: Vec<Vec<&Attribute>> = input
        .variants
        .iter()
        .map(|v| {
            v.attrs
                .iter()
                .filter(|v| v.path().is_ident("cfg"))
                .collect()
        })
        .collect();

    // generate unbound arg params for each enum variant
    let variants_args = nested_attrs
        .iter()
        .map(|(fields, nested_attr)| match &fields {
            syn::Fields::Named(_) => {
                if let Some((field, _)) = nested_attr {
                    let key = field.ident.as_ref().expect("field missing ident");
                    quote!({ #key, .. })
                } else {
                    quote!({ .. })
                }
            }
            syn::Fields::Unnamed(_) => {
                let args = fields.iter().map(|field| {
                    if field
                        .attrs
                        .iter()
                        .any(|attr| attr.path().is_ident("nested"))
                    {
                        quote!(nested)
                    } else {
                        quote!(_)
                    }
                });
                quote! {
                    (#( #args, )*)
                }
            }
            syn::Fields::Unit => quote!(),
        });

    // generate incr calls for each variant, if a field is marked with `#[nested]`,
    // the incr is instead delegated there
    let variant_incr_call =
        nested_attrs
            .iter()
            .zip(snake_variants.iter())
            .map(|((_, nested_attr), ident)| {
                if let Some((field, attr)) = nested_attr {
                    let inner_val_ident = field
                        .ident
                        .clone()
                        .unwrap_or_else(|| Ident::new("nested", attr.bracket_token.span.span()));
                    quote! {{
                        self.#ident.incr(#inner_val_ident);
                    }}
                } else {
                    quote!(self.#ident.incr())
                }
            });

    let skip_cleared = attrs.skip_cleared;
    let serializer = nested_attrs.iter().map(|(_, nested_attr)| {
        if skip_cleared && nested_attr.is_none() {
            quote!("metered::error_variant_serializer_skip_cleared")
        } else {
            quote!("metered::error_variant_serializer")
        }
    });

    Ok(quote! {
        #input

        #[derive(serde::Serialize, Default, Debug)]
        #[allow(missing_docs)]
        #vis struct #metrics_ident<C: metered::metric::Counter = metered::atomic::AtomicInt<u64>> {
            #[serde(skip)]
            __phantom: std::marker::PhantomData<C>,
            #(
                #(#cfg_attrs)*
                #[serde(rename = #stringified_variants, serialize_with = #serializer)]
                pub #snake_variants: #metric_type,
            )*
        }

        impl<C: metered::metric::Counter> metered::ErrorBreakdownIncr<#ident> for #metrics_ident<C> {
            fn incr(&self, err: &#ident) {
                match err {
                    #( #(#cfg_attrs)* #ident::#variants #variants_args => #variant_incr_call, )*
                }
            }
        }

        impl<C: metered::metric::Counter> metered::clear::Clear for #metrics_ident<C> {
            fn clear(&self) {
                #( #(#cfg_attrs)* self.#snake_variants.clear(); )*
            }
        }

        impl<T, C: metered::metric::Counter> metered::metric::Metric<Result<T, #ident>> for #metrics_ident<C> {}

        impl<C: metered::metric::Counter> metered::metric::Enter for #metrics_ident<C> {
            type E = ();
            fn enter(&self) {}
        }

        impl<T, C: metered::metric::Counter> metered::metric::OnResult<Result<T, #ident>> for #metrics_ident<C> {
            fn on_result(&self, (): (), r: &Result<T, #ident>) -> metered::metric::Advice {
                if let Err(e) = r {
                    metered::ErrorBreakdownIncr::incr(self, e);
                }
                metered::metric::Advice::Return
            }
        }

        impl<C: metered::metric::Counter> metered::ErrorBreakdown<C> for #ident {
            type ErrorCount = #metrics_ident<C>;
        }
    }.into())
}

type FieldWithNestedAttribute = Option<(Field, Attribute)>;

/// Gets all variants from the given `ItemEnum`, and returns `Some(Field,
/// Attribute)` along with each variant if one of fields contained a `#[nested]`
/// attribute.
///
/// If a `#[nested]` attribute is found, then the attribute itself removed from
/// `input` so that we don't get "unrecognised attribute" errors.
fn get_nested_attrs(input: &mut ItemEnum) -> syn::Result<Vec<(Fields, FieldWithNestedAttribute)>> {
    let attrs = input
        .variants
        .iter_mut()
        .map(|v| {
            // clone fields before we do any mutation on it so consumers can figure out the
            // position of #[nested] fields.
            let fields = v.fields.clone();

            let inner_fields = match &mut v.fields {
                syn::Fields::Named(v) => &mut v.named,
                syn::Fields::Unnamed(v) => &mut v.unnamed,
                _ => return Ok((fields, None)),
            };

            // field containing the nested attribute, along with the attribute itself
            let mut nested_attr = None;

            for field in inner_fields {
                if let Some(pos) = field.attrs.iter().position(|a| a.path().is_ident("nested")) {
                    let attr = field.attrs.remove(pos);

                    // if we've already found a nested attribute on a field in the current variant,
                    // throw an error
                    if nested_attr.is_some() {
                        return Err(syn::Error::new(
                            attr.bracket_token.span.span(),
                            "Can't declare `#[nested]` on more than one field in a single variant",
                        ));
                    }

                    nested_attr = Some((field.clone(), attr.clone()));
                }
            }

            Ok((fields, nested_attr))
        })
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(attrs)
}
