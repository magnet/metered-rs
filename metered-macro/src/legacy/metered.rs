//! The module supporting #[metered]

use proc_macro::TokenStream;

use super::{measure_opts::MeasureRequestAttribute, metered_opts::MeteredKeyValAttribute};

use aspect_weave::*;
use std::rc::Rc;
use synattra::ParseAttributes;

pub fn metered(attrs: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let woven_impl_block = weave_impl_block::<MeteredWeave>(attrs, item)?;

    // The woven impl block may carry `#[metric(rename = "...")]` on measured
    // methods, controlling the wire-name segment a method contributes (the
    // registry field keeps the method name). Strip those helper attributes
    // before re-emitting the block and record the renames.
    let mut impl_block = woven_impl_block.woven_block.clone();
    let mut method_renames: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for item in &mut impl_block.items {
        if let syn::ImplItem::Method(method) = item {
            if let Some(segment) = take_metric_rename(method)? {
                method_renames.insert(method.sig.ident.to_string(), segment);
            }
        }
    }

    let metered = &woven_impl_block.main_attributes.to_metered();
    let measured = &woven_impl_block.woven_fns;
    let registry_name = &metered.registry_name;
    let registry_ident = &metered.registry_ident;
    let visibility = &metered.visibility;

    let mut code = quote! {};

    // Generated code is serde-free; it exposes only through `MetricTree`.
    let registry_derives = quote! { #[derive(Debug, Default)] };

    // Top-level registry: one field per measured method, each holding that
    // method's sub-registry. The wire segment is the `#[metric(rename)]` value
    // if present, otherwise the method name.
    let mut reg_fields = quote! {};
    let mut reg_children: Vec<RegistryChild> = Vec::new();
    for (fun_name, _) in measured.iter() {
        let fun_registry_ident =
            fun_registry_ident(registry_name, fun_name, impl_block.impl_token.span);
        let segment = method_renames
            .get(&fun_name.to_string())
            .cloned()
            .unwrap_or_else(|| fun_name.to_string());
        reg_fields = quote! { #reg_fields pub #fun_name : #fun_registry_ident, };
        reg_children.push(RegistryChild {
            access: quote! { &self.#fun_name },
            segment: quote! { #segment },
        });
    }
    let top_registry = registry_def(
        &registry_derives,
        visibility,
        registry_ident,
        &reg_fields,
        &reg_children,
    );
    code = quote! { #code #top_registry };

    // Per-method registries: one field per measured metric, named after it.
    for (fun_name, measure_request_attrs) in measured.iter() {
        let fun_registry_ident =
            fun_registry_ident(registry_name, fun_name, impl_block.impl_token.span);
        let mut fun_fields = quote! {};
        let mut fun_children: Vec<RegistryChild> = Vec::new();
        for measure_req_attr in measure_request_attrs.iter() {
            for metric in measure_req_attr.to_requests().iter() {
                let metric_field = metric.ident();
                let metric_type = metric.type_path();
                fun_fields = quote! { #fun_fields pub #metric_field : #metric_type, };
                fun_children.push(RegistryChild {
                    access: quote! { &self.#metric_field },
                    segment: quote! { stringify!(#metric_field) },
                });
            }
        }
        let fun_registry = registry_def(
            &registry_derives,
            visibility,
            &fun_registry_ident,
            &fun_fields,
            &fun_children,
        );
        code = quote! { #code #fun_registry };
    }

    code = quote! {
        #impl_block

        #code
    };

    let result: TokenStream = code.into();
    Ok(result)
}

/// One child of a generated registry: how to reach the field, and the wire-name
/// segment it contributes under the registry's name.
struct RegistryChild {
    access: proc_macro2::TokenStream,
    segment: proc_macro2::TokenStream,
}

/// Builds the per-method sub-registry identifier, e.g. `ApiMetricsHandle`.
fn fun_registry_ident(
    registry_name: &str,
    fun_name: &syn::Ident,
    span: proc_macro2::Span,
) -> syn::Ident {
    use heck::ToUpperCamelCase;
    let name = format!(
        "{}{}",
        registry_name,
        fun_name.to_string().to_upper_camel_case()
    );
    syn::Ident::new(&name, span)
}

/// Generates a registry struct plus its `MetricTree` impl. The top-level and
/// per-method registries share this shape: a struct of fields, and a
/// describe/collect that forwards to each child under its name segment.
fn registry_def(
    derives: &proc_macro2::TokenStream,
    visibility: &syn::Visibility,
    ident: &syn::Ident,
    fields: &proc_macro2::TokenStream,
    children: &[RegistryChild],
) -> proc_macro2::TokenStream {
    let describe = children.iter().map(|RegistryChild { access, segment }| {
        quote! {
            ::metered_semantic::MetricTree::describe(
                #access,
                &::metered_semantic::join_name(name, #segment),
                labels,
                schema,
            );
        }
    });
    let collect = children.iter().map(|RegistryChild { access, segment }| {
        quote! {
            ::metered_semantic::MetricTree::collect(
                #access,
                &::metered_semantic::join_name(name, #segment),
                labels,
                values,
            );
        }
    });
    let housekeep = children.iter().map(|RegistryChild { access, .. }| {
        quote! { ::metered_semantic::MetricTree::housekeep(#access); }
    });
    let needs_housekeep = children.iter().map(|RegistryChild { access, .. }| {
        quote! { || ::metered_semantic::MetricTree::needs_housekeep(#access) }
    });
    quote! {
        #derives
        #[allow(missing_docs)]
        #visibility struct #ident {
            #fields
        }

        impl ::metered_semantic::MetricTree for #ident {
            fn describe(
                &self,
                name: &str,
                labels: &[(&str, &str)],
                schema: &mut ::metered_semantic::MetricSchema,
            ) {
                #(#describe)*
            }

            fn collect(
                &self,
                name: &str,
                labels: &[(&str, &str)],
                values: &mut ::metered_semantic::MetricValues,
            ) {
                #(#collect)*
            }

            fn housekeep(&self) {
                #(#housekeep)*
            }

            fn needs_housekeep(&self) -> bool {
                false #(#needs_housekeep)*
            }
        }
    }
}

/// Extracts and removes a `#[metric(rename = "...")]` attribute from a measured
/// method, returning the wire-name segment it specifies. Only `rename` is
/// supported on methods (a method's metrics are a sub-registry, so `flatten`
/// does not apply).
fn take_metric_rename(method: &mut syn::ImplItemMethod) -> syn::Result<Option<String>> {
    let attr = crate::metric_attr::parse(&method.attrs)?;
    if attr.flatten {
        return Err(syn::Error::new_spanned(
            &method.sig.ident,
            "`#[metric(flatten)]` is not supported on a measured method; only `rename` is",
        ));
    }
    method.attrs.retain(|attr| !attr.path.is_ident("metric"));
    Ok(attr.rename)
}

struct MeteredWeave;
impl Weave for MeteredWeave {
    type MacroAttributes = MeteredKeyValAttribute;

    fn update_fn_block(
        item_fn: &syn::ImplItemMethod,
        main_attr: &Self::MacroAttributes,
        fn_attr: &[Rc<<Self as ParseAttributes>::Type>],
    ) -> syn::Result<syn::Block> {
        let metered = main_attr.to_metered();
        let ident = &item_fn.sig.ident;
        let block = &item_fn.block;
        // We must alter the block to capture early returns
        // using a closure, and handle the async case.

        let outer_block = if item_fn.sig.asyncness.is_some() {
            // Run the body as a *borrowing* async block, awaited in place. A
            // `return` inside the block exits the block (so the metric is still
            // recorded afterwards), and `self` is borrowed -- not moved into a
            // future -- so the body may take `&mut self` and the metric can be
            // re-borrowed afterwards to record (issue #13).
            //
            // `.await` is built via `parse_str` rather than `quote!` to avoid
            // emitting the `await` keyword token directly.
            let await_expr = syn::parse_str::<syn::Expr>("__metered_fut.await")?;
            quote! {
                {
                    let __metered_fut = async #block;
                    #await_expr
                }
            }
        } else {
            // A non-`move` closure: it borrows `self` only for the duration of
            // the call (released before the metric is re-borrowed to record),
            // while still moving any owned locals the body consumes. This is
            // what lets a measured sync body take `&mut self` (issue #13).
            quote! {
                (|| #block)()
            }
        };

        let r = measure_list(&metered.registry_expr, ident, fn_attr, outer_block);

        let new_block = syn::parse2::<syn::Block>(r)?;
        Ok(new_block)
    }
}
impl ParseAttributes for MeteredWeave {
    type Type = MeasureRequestAttribute;

    // const
    fn fn_attr_name() -> &'static str {
        "measure"
    }
}

fn measure_list(
    registry_expr: &syn::Expr,
    fun_ident: &syn::Ident,
    measure_request_attrs: &[Rc<MeasureRequestAttribute>],
    mut inner: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    // Inline the metric path expression directly into `measure!` (for both sync
    // and async bodies). Because `measure!` is two-phase, the metric -- and thus
    // `self` -- is borrowed only briefly to enter and again to record, never
    // across the body, so the body is free to take `&mut self` (issue #13).
    for measure_req_attr in measure_request_attrs.iter() {
        for metric in measure_req_attr.to_requests().iter() {
            let metric_field = metric.ident();
            inner = quote! {
                ::metered_semantic::measure! { &#registry_expr.#fun_ident.#metric_field, #inner }
            };
        }
    }

    // Add final braces
    quote! {
        {
            #inner
        }
    }
}
