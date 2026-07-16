use super::error_count_opts::ErrorCountKeyValAttribute;
use heck::ToSnakeCase;
use proc_macro::TokenStream;
use syn::{Attribute, Field, Fields, Ident, ItemEnum};

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
                        |_| quote!(<#error_type as ::metered_semantic::ErrorBreakdown>::ErrorCount),
                    )
            } else {
                quote!(::metered_semantic::handle::Handle<::std::sync::atomic::AtomicU64>)
            }
        })
        .collect::<Vec<_>>();

    let ident = &input.ident;

    let variants = input.variants.iter().map(|v| &v.ident);
    let snake_variants: Vec<Ident> = input
        .variants
        .iter()
        .map(|v| Ident::new(&v.ident.to_string().to_snake_case(), v.ident.span()))
        .collect();
    let variant_names: Vec<String> = input.variants.iter().map(|v| v.ident.to_string()).collect();

    // copy #[cfg(..)] attributes from the variant and apply them to the
    // corresponding error in our struct so we don't point to an invalid variant
    // in certain configurations.
    let cfg_attrs: Vec<Vec<&Attribute>> = input
        .variants
        .iter()
        .map(|v| v.attrs.iter().filter(|v| v.path.is_ident("cfg")).collect())
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
                    if field.attrs.iter().any(|attr| attr.path.is_ident("nested")) {
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
                        .unwrap_or_else(|| Ident::new("nested", attr.bracket_token.span));
                    quote! {{
                        self.#ident.incr(#inner_val_ident);
                    }}
                } else {
                    quote!(::metered_semantic::Counter::incr(&self.#ident))
                }
            });

    // OpenMetrics exposition: flat variants become `error_kind`-labelled samples
    // on a single counter family; nested breakdowns recurse under their own
    // name segment. Rendering goes through the default `MetricTree::encode`
    // (schema + values), so only `describe` and `collect` are generated.
    let mut breakdown_describe = proc_macro2::TokenStream::new();
    let mut breakdown_collect = proc_macro2::TokenStream::new();
    let mut has_flat_variants = false;
    for (((snake, variant_name), cfg), (_, nested)) in snake_variants
        .iter()
        .zip(variant_names.iter())
        .zip(cfg_attrs.iter())
        .zip(nested_attrs.iter())
    {
        if nested.is_none() {
            has_flat_variants = true;
        }

        let describe_line = if nested.is_some() {
            quote! {
                ::metered_semantic::MetricTree::describe(
                    &self.#snake,
                    &::metered_semantic::join_name(name, stringify!(#snake)),
                    labels,
                    schema,
                );
            }
        } else {
            quote! {}
        };
        breakdown_describe.extend(quote! { #(#cfg)* #describe_line });

        let collect_line = if nested.is_some() {
            quote! {
                ::metered_semantic::MetricTree::collect(
                    &self.#snake,
                    &::metered_semantic::join_name(name, stringify!(#snake)),
                    labels,
                    values,
                );
            }
        } else {
            quote! {
                {
                    let mut __labels: ::std::vec::Vec<(&str, &str)> = labels.to_vec();
                    __labels.push(("error_kind", #variant_name));
                    values.sample(
                        &::std::format!("{}_total", name),
                        &__labels,
                        ::metered_semantic::CounterSource::get(&self.#snake),
                    );
                }
            }
        };
        breakdown_collect.extend(quote! { #(#cfg)* #collect_line });
    }

    let flat_breakdown_describe = if has_flat_variants {
        quote! {
            let mut __labels: ::std::vec::Vec<(&str, &str)> = labels.to_vec();
            __labels.push(("error_kind", ""));
            schema.add_family(name, ::metered_semantic::MetricType::Counter, &__labels);
        }
    } else {
        quote! {}
    };

    // Generated code is serde-free; it exposes only through `MetricTree`.
    let struct_derives = quote! { #[derive(Default, Debug, Clone)] };

    let mut struct_fields = proc_macro2::TokenStream::new();
    for (((snake, _variant_name), cfg), ((_, _nested), metric_ty)) in snake_variants
        .iter()
        .zip(variant_names.iter())
        .zip(cfg_attrs.iter())
        .zip(nested_attrs.iter().zip(metric_type.iter()))
    {
        struct_fields.extend(quote! {
            #(#cfg)*
            pub #snake: #metric_ty,
        });
    }

    Ok(quote! {
        #input

        #struct_derives
        #[allow(missing_docs)]
        #vis struct #metrics_ident {
            #struct_fields
        }

        impl ::metered_semantic::ErrorBreakdownIncr for #metrics_ident {
            type Error = #ident;

            fn incr(&self, err: &#ident) {
                match err {
                    #( #(#cfg_attrs)* #ident::#variants #variants_args => #variant_incr_call, )*
                }
            }
        }

        impl ::metered_semantic::metric::Measure for #metrics_ident {
            type Recorder = ::metered_semantic::ErrorBreakdownRecorder<#metrics_ident>;

            fn enter(&self) -> Self::Recorder {
                // The breakdown's counters are shared handles, so cloning the
                // struct is cheap and yields an owned handle the recorder keeps
                // across the measured expression.
                ::metered_semantic::ErrorBreakdownRecorder::new(self.clone())
            }
        }

        impl ::metered_semantic::ErrorBreakdown for #ident {
            type ErrorCount = #metrics_ident;
        }

        impl ::metered_semantic::MetricTree for #metrics_ident {
            fn describe(
                &self,
                name: &str,
                labels: &[(&str, &str)],
                schema: &mut ::metered_semantic::MetricSchema,
            ) {
                #flat_breakdown_describe
                #breakdown_describe
            }

            fn collect(
                &self,
                name: &str,
                labels: &[(&str, &str)],
                values: &mut ::metered_semantic::MetricValues,
            ) {
                #breakdown_collect
            }
        }
    }
    .into())
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
                if let Some(pos) = field.attrs.iter().position(|a| a.path.is_ident("nested")) {
                    let attr = field.attrs.remove(pos);

                    // if we've already found a nested attribute on a field in the current variant,
                    // throw an error
                    if nested_attr.is_some() {
                        return Err(syn::Error::new(
                            attr.bracket_token.span,
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
