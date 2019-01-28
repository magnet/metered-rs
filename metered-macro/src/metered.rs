use proc_macro::TokenStream;

use syn::parse_macro_input;

use crate::measure_opts::MeasureRequestAttribute;
use crate::metered_opts::MeteredKeyValAttribute;

pub fn metered(attrs: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attrs as MeteredKeyValAttribute);

    let metered = attrs.to_metered();

    let mut parsed_input: syn::ItemImpl = parse_macro_input!(item);

    let registry_name = metered.registry;
    let registry_expr = "self.metrics";
    let mut measured = indexmap::map::IndexMap::new();

    for item in parsed_input.items.iter_mut() {
        if let syn::ImplItem::Method(item_fn) = item {
            let attrs = &mut item_fn.attrs;

            let (ours, theirs): (Vec<syn::Attribute>, Vec<syn::Attribute>) = attrs
                .clone()
                .into_iter()
                .partition(|attr| attr.path.is_ident("measure"));

            item_fn.attrs = theirs;

            let mut measure_reqs: Vec<MeasureRequestAttribute> = Vec::new();
            for attr in ours.into_iter() {
                let tts: TokenStream = attr.tts.into();
                let p = parse_macro_input!(tts as MeasureRequestAttribute);
                measure_reqs.push(p);
            }

            if measure_reqs.is_empty() {
                continue;
            }

            let block = &item_fn.block;
            let ident = &item_fn.sig.ident;
            let qualified_registry_name = format!("{}.{}", registry_expr, &ident);

            let r: proc_macro::TokenStream = measure_list(
                &qualified_registry_name,
                &measure_reqs,
                quote! { #block }.into(),
            )
            .into();

            let new_block: syn::Block = syn::parse(r).expect("block");
            item_fn.block = new_block;

            measured.insert(&item_fn.sig.ident, measure_reqs);
        }
    }

    let mut code = quote! {};

    let mut reg_fields = quote! {};
    for (fun_name, _) in measured.iter() {
        use heck::CamelCase;
        let fun_reg_name = format!("{}{}", registry_name, fun_name.to_string().to_camel_case());
        let fun_registry_ident = syn::Ident::new(&fun_reg_name, parsed_input.impl_token.span);

        reg_fields = quote! {
            #reg_fields
            #fun_name : #fun_registry_ident,
        }
    }

    code = quote! {
        #code

        #[derive(Debug, Default)]
        struct #registry_name {
            #reg_fields
        }
    };

    drop(reg_fields);

    for (fun_name, measure_request_attrs) in measured.iter() {
        use heck::CamelCase;
        let fun_reg_name = format!("{}{}", registry_name, fun_name.to_string().to_camel_case());
        let fun_registry_ident = syn::Ident::new(&fun_reg_name, parsed_input.impl_token.span);

        let mut fun_reg_fields = quote! {};

        for measure_req_attr in measure_request_attrs.iter() {
            let metric_requests = measure_req_attr.to_requests();

            for metric in metric_requests.iter() {
                let metric_field = metric.ident();
                let metric_type = metric.type_path();

                fun_reg_fields = quote! {
                    #fun_reg_fields
                    #metric_field : #metric_type,
                }
            }
        }

        code = quote! {
            #code

            #[derive(Debug, Default)]
            struct #fun_registry_ident {
                #fun_reg_fields
            }
        };
    }

    code = quote! {
        #parsed_input

        #code
    };

    let result: TokenStream = code.into();
    // println!("Result {}", result.to_string());
    result
}

fn measure_list<'a>(
    qualified_registry_name: &'a str,
    measure_request_attrs: &[MeasureRequestAttribute],
    expr: TokenStream,
) -> TokenStream {
    let registry = syn::parse_str::<syn::Expr>(qualified_registry_name).unwrap();

    let mut inner: proc_macro2::TokenStream = expr.into();

    // Recursive macro invocations
    for measure_req_attr in measure_request_attrs.iter() {
        let metric_requests = measure_req_attr.to_requests();

        for metric in metric_requests.iter() {
            let metric_var = metric.ident();
            inner = quote! {
                measure! { #metric_var, #inner }
            };
        }
    }

    // Let-bindings to avoid moving issues
    for measure_req_attr in measure_request_attrs.iter() {
        let metric_requests = measure_req_attr.to_requests();

        for metric in metric_requests.iter() {
            let metric_var = syn::Ident::new(&metric.field_name, proc_macro2::Span::call_site());

            inner = quote! {
                let #metric_var = &#registry.#metric_var;
                #inner
            };
        }

        // // Use debug routine if enabled!
        // if let Some(opt) = metric.debug {
        // }
    }

    // Add final braces
    inner = quote! {
        {
            #inner
        }
    };

    inner.into()
}
