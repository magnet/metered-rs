//! `#[derive(LabelSet)]` and `#[derive(MetricTree)]`.

use proc_macro::TokenStream;
use proc_macro2::TokenTree;
use quote::{ToTokens, format_ident, quote};
use std::collections::HashSet;
use syn::{Data, DeriveInput, Field, Fields, Type};

/// Collects the named fields of a struct, or returns a compile error token
/// stream describing why it could not.
fn named_fields(input: &DeriveInput, derive: &str) -> Result<Vec<Field>, TokenStream> {
    let data = match &input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(error(
                &input.ident,
                &format!("`{derive}` can only be derived for structs"),
            ));
        }
    };
    match &data.fields {
        Fields::Named(named) => Ok(named.named.iter().cloned().collect()),
        _ => Err(error(
            &input.ident,
            &format!("`{derive}` requires a struct with named fields"),
        )),
    }
}

fn error(ident: &syn::Ident, message: &str) -> TokenStream {
    syn::Error::new_spanned(ident, message)
        .to_compile_error()
        .into()
}

/// Whether `ty` is textually one of the standard string shapes whose stored
/// text can be lent to the label visitor as `&str` directly: `String`, `str`
/// behind any references (`&str`, `&'static str`), `Arc<str>`, and
/// `Cow<'_, str>`. A proc macro cannot resolve types, so this is a
/// conservative last-segment name check (like [`is_ambiguous_atomic`]): any
/// unrecognized spelling keeps the allocating `ToString` path, which is
/// always correct.
fn lends_as_str(ty: &Type) -> bool {
    match ty {
        Type::Reference(reference) => lends_as_str(reference.elem.as_ref()),
        Type::Path(path) => {
            let Some(segment) = path.path.segments.last() else {
                return false;
            };
            match segment.ident.to_string().as_str() {
                "String" | "str" => segment.arguments.is_empty(),
                "Arc" | "Cow" => sole_type_argument_is_str(segment),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Whether `segment`'s generic arguments carry exactly one *type* argument
/// and it is a bare `str` (lifetimes are ignored, so `Cow<'a, str>` counts).
fn sole_type_argument_is_str(segment: &syn::PathSegment) -> bool {
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return false;
    };
    let type_args: Vec<&Type> = args
        .args
        .iter()
        .filter_map(|arg| match arg {
            syn::GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .collect();
    matches!(
        type_args.as_slice(),
        [Type::Path(path)] if path.path.is_ident("str")
    )
}

/// Derives [`metered::LabelSet`]: each field becomes a label whose name is the
/// field name.
///
/// The label value is the field's `Display` rendering
/// (`ToString::to_string`), except for fields whose type is textually a
/// standard string shape (`String`, `&str`, `Arc<str>`, `Cow<'_, str>`):
/// those **lend** the stored string to the visitor directly, so encoding a
/// string-labeled key allocates nothing -- the point of the
/// `for_each_label` visitor. Both paths render identical bytes.
///
/// Container-level `#[metrics(crate = "...")]` overrides the emitted runtime
/// path (default `::metered`). Other `#[metrics(...)]` options are rejected
/// here -- they shape `MetricTree` only.
pub fn label_set(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse(input) {
        Ok(input) => input,
        Err(e) => return e.to_compile_error().into(),
    };
    let fields = match named_fields(&input, "LabelSet") {
        Ok(fields) => fields,
        Err(err) => return err,
    };
    let container = match crate::metric_attr::parse_container(&input.attrs) {
        Ok(container) => container,
        Err(err) => return err.to_compile_error().into(),
    };
    if container.prefix.is_some()
        || container.help.is_some()
        || container.unit.is_some()
        || !container.labels.is_empty()
    {
        return syn::Error::new_spanned(
            &input.ident,
            "`#[derive(LabelSet)]` only honors `#[metrics(crate = \"...\")]`; \
             `prefix` / `help` / `unit` / `label(...)` shape `MetricTree`",
        )
        .to_compile_error()
        .into();
    }
    let metered = match &container.krate {
        Some(path) => quote! { #path },
        None => quote! { ::metered },
    };
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let field_names: Vec<_> = fields
        .iter()
        .map(|field| field.ident.as_ref().expect("named field has an ident"))
        .collect();
    let field_values = fields.iter().zip(&field_names).map(|(field, name)| {
        if lends_as_str(&field.ty) {
            // The stored string is lent as-is: no `ToString` allocation on
            // the visit path. `AsRef<str>` covers every shape
            // `lends_as_str` admits (including behind references).
            quote! { ::core::convert::AsRef::<str>::as_ref(&self.#name) }
        } else {
            quote! { &::std::string::ToString::to_string(&self.#name) }
        }
    });

    quote! {
        impl #impl_generics #metered::LabelSet for #ident #ty_generics #where_clause {
            fn for_each_label(&self, f: &mut dyn ::std::ops::FnMut(&str, &str)) {
                #(
                    f(stringify!(#field_names), #field_values);
                )*
            }

            fn label_names(out: &mut ::std::vec::Vec<::std::string::String>) {
                #(
                    out.push(::std::string::String::from(stringify!(#field_names)));
                )*
            }
        }
    }
    .into()
}

/// Derives [`metered::MetricTree`]: exposes each field as a sub-tree whose name
/// is `<name>_<field>`, delegating to the field's own `MetricTree`.
pub fn metric_tree(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse(input) {
        Ok(input) => input,
        Err(e) => return e.to_compile_error().into(),
    };
    let fields = match named_fields(&input, "MetricTree") {
        Ok(fields) => fields,
        Err(err) => return err,
    };
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let container = match crate::metric_attr::parse_container(&input.attrs) {
        Ok(container) => container,
        Err(err) => return err.to_compile_error().into(),
    };
    // Path the expansion uses to reach the metered runtime. Defaults to
    // `::metered`; `#[metrics(crate = "...")]` overrides it (facade renames,
    // or a local re-export module), mirroring `#[serde(crate = "...")]`.
    let metered = match &container.krate {
        Some(path) => quote! { #path },
        None => quote! { ::metered },
    };
    // A container `#[metrics(prefix = ...)]` joins a segment onto the inherited
    // name; a container `#[metrics(label(...))]` appends constant labels. Both
    // compose when the tree is nested, and let a root tree carry its own
    // prefix/labels without a hand-wired `Registry`.
    let prefix_apply = match &container.prefix {
        Some(prefix) => quote! {
            let name: &str = &#metered::join_name(name, #prefix);
        },
        None => quote! {},
    };
    let labels_apply = if container.labels.is_empty() {
        quote! {}
    } else {
        let pairs = container
            .labels
            .iter()
            .map(|(key, value)| quote! { (#key, #value) });
        // Composition funnels through `compose_labels` so a container label
        // that collides with an inherited (e.g. registry) label resolves
        // inner-wins -- one pair per name -- instead of duplicating the name.
        quote! {
            let labels: &[(&str, &str)] =
                &#metered::compose_labels(labels, [ #(#pairs),* ]);
        }
    };
    let meta_help = match &container.help {
        Some(help) => quote! {
            fn help() -> ::std::option::Option<#metered::Help> {
                ::std::option::Option::Some(#metered::Help::from(#help))
            }
        },
        None => quote! {},
    };
    let meta_unit = match &container.unit {
        Some(unit) => quote! {
            fn unit() -> ::std::option::Option<#metered::Unit> {
                ::std::option::Option::Some(#metered::Unit::from(#unit))
            }
        },
        None => quote! {},
    };

    let generic_params = generic_param_names(&input.generics);

    let mut view_helpers = Vec::new();
    let mut describe_stmts = Vec::new();
    let mut collect_stmts = Vec::new();
    let mut housekeep_stmts = Vec::new();
    let mut needs_housekeep_terms = Vec::new();
    for field in &fields {
        let field_ident = field.ident.as_ref().expect("named field has an ident");
        // Borrowed fields are already references; owned fields need `&`.
        let field_value = if matches!(field.ty, Type::Reference(_)) {
            quote! { self.#field_ident }
        } else {
            quote! { &self.#field_ident }
        };
        let field_target_ty = match &field.ty {
            Type::Reference(reference) => reference.elem.as_ref(),
            ty => ty,
        };

        let plan = match field_plan(field) {
            Ok(plan) => plan,
            Err(err) => return err.to_compile_error().into(),
        };
        let Some(plan) = plan else {
            continue;
        };

        // The family name this field contributes, so per-field `help`/`unit`
        // attach to the exact series. A flattened field emits under the
        // inherited `name`; a segment joins onto it.
        let family_name = match &plan.shape {
            FieldShape::Flatten => quote! { name },
            FieldShape::Segment(segment) => quote! { &#metered::join_name(name, #segment) },
        };
        // Metadata must be set before `describe` so the family picks it up as it
        // is created.
        let help_stmt = match &plan.help {
            Some(help) => quote! { schema.set_help_for(#family_name, #help); },
            None => quote! {},
        };
        let unit_stmt = match &plan.unit {
            Some(unit) => quote! { schema.set_unit_for(#family_name, #unit); },
            None => quote! {},
        };

        match plan.kind {
            FieldKind::Tree | FieldKind::Gauge | FieldKind::Counter | FieldKind::Info => {
                // `gauge`/`counter`/`info` force the leaf's OpenMetrics type by
                // wrapping the field in the public `AsGauge` / `AsCounter` /
                // `AsInfo` adapters; without them the field's own `Metric` impl
                // decides (e.g. `AtomicU64` defaults to a counter). `info`
                // fields only promise `T: Info`, so `AsInfo` is what lets them
                // ride the same `Renamed`/`Flatten` leaf path as every other
                // field kind.
                let node_inner = match plan.kind {
                    FieldKind::Tree => quote! { #field_value },
                    FieldKind::Gauge => quote! { &#metered::AsGauge::from(#field_value) },
                    FieldKind::Counter => quote! { &#metered::AsCounter::from(#field_value) },
                    FieldKind::Info => quote! { &#metered::AsInfo::from(#field_value) },
                    FieldKind::View => unreachable!(),
                };
                // Wrap the field so its name segment is shaped (renamed, or
                // flattened away). The same shaped node drives both describe and
                // collect.
                let node = match &plan.shape {
                    FieldShape::Flatten => quote! { &#metered::Flatten::new(#node_inner) },
                    FieldShape::Segment(segment) => {
                        quote! { &#metered::Renamed::new(#segment, #node_inner) }
                    }
                };

                describe_stmts.push(quote! {
                    #help_stmt
                    #unit_stmt
                    #metered::MetricTree::describe(#node, name, labels, schema);
                });
                collect_stmts.push(quote! {
                    #metered::MetricTree::collect(#node, name, labels, values);
                });
                if matches!(plan.kind, FieldKind::Info) {
                    // An `info` field only promises `T: Info` (not
                    // `MetricTree`), and info metadata has no structural
                    // upkeep: nothing to forward (same asymmetry as
                    // `AsInfo`'s default `Metric::housekeep`).
                    housekeep_stmts.push(quote! {});
                    needs_housekeep_terms.push(quote! {});
                } else {
                    // Housekeeping ignores name shaping and leaf-type wrapping,
                    // so forward straight to the field.
                    housekeep_stmts.push(quote! {
                        #metered::MetricTree::housekeep(#field_value);
                    });
                    needs_housekeep_terms.push(quote! {
                        || #metered::MetricTree::needs_housekeep(#field_value)
                    });
                }
            }
            FieldKind::View => {
                // The component's view layout is context-free (a
                // `MetricTreeView<'static, _>` owns its prefix, labels, and
                // entries), so it is built once per process and cached in a
                // hidden `static` instead of being rebuilt by every
                // describe/collect/housekeep/needs_housekeep call. A `static`
                // cannot mention generic parameters, so a component type that
                // involves the struct's generics falls back to per-call
                // construction. The check is token-textual and conservative: a
                // false positive only costs the cache, never soundness.
                let cacheable = generic_params.is_empty()
                    || !tokens_mention_params(field_target_ty.to_token_stream(), &generic_params);
                let view_binding = if cacheable {
                    let helper = format_ident!("__metered_cached_view_{}", field_ident);
                    view_helpers.push(quote! {
                        fn #helper() -> &'static #metered::MetricTreeView<'static, #field_target_ty> {
                            static VIEW: ::std::sync::OnceLock<
                                #metered::MetricTreeView<'static, #field_target_ty>,
                            > = ::std::sync::OnceLock::new();
                            VIEW.get_or_init(
                                <#field_target_ty as #metered::MetricsView>::metrics_view,
                            )
                        }
                    });
                    quote! { let __view = #helper(); }
                } else {
                    quote! {
                        let __view =
                            <#field_target_ty as #metered::MetricsView>::metrics_view();
                    }
                };

                let describe_stmt = match &plan.shape {
                    FieldShape::Flatten => quote! {
                        #view_binding
                        __view.describe_prefixed(#field_value, Some(name), labels, schema);
                    },
                    FieldShape::Segment(segment) => quote! {
                        #view_binding
                        let __parent = #metered::join_name(name, #segment);
                        __view.describe_prefixed(#field_value, Some(__parent.as_str()), labels, schema);
                    },
                };
                let collect_stmt = match &plan.shape {
                    FieldShape::Flatten => quote! {
                        #view_binding
                        __view.collect_prefixed(#field_value, Some(name), labels, values);
                    },
                    FieldShape::Segment(segment) => quote! {
                        #view_binding
                        let __parent = #metered::join_name(name, #segment);
                        __view.collect_prefixed(#field_value, Some(__parent.as_str()), labels, values);
                    },
                };
                describe_stmts.push(describe_stmt);
                collect_stmts.push(collect_stmt);
                housekeep_stmts.push(quote! {
                    #view_binding
                    __view.housekeep_entries(#field_value);
                });
                // Mirror the housekeep routing above: a view field must OR its
                // entries' upkeep into the tree's `needs_housekeep`, or a metric
                // that lives only behind a bare `#[metrics]` field (e.g. a
                // dynamic exponential histogram) would never be housekept and
                // would freeze once saturated.
                needs_housekeep_terms.push(quote! {
                    || {
                        #view_binding
                        __view.needs_housekeep_entries(#field_value)
                    }
                });
            }
        }
    }

    // The impls live inside an anonymous `const` so the cached-view helper
    // fns can be shared by all four trait methods without leaking names into
    // the user's module.
    quote! {
        const _: () = {
        #( #view_helpers )*

        impl #impl_generics #metered::MetricTree for #ident #ty_generics #where_clause {
            fn describe(
                &self,
                name: &str,
                labels: &[(&str, &str)],
                schema: &mut #metered::MetricSchema,
            ) {
                #prefix_apply
                #labels_apply
                #( #describe_stmts )*
            }

            fn collect(
                &self,
                name: &str,
                labels: &[(&str, &str)],
                values: &mut #metered::MetricValues,
            ) {
                #prefix_apply
                #labels_apply
                #( #collect_stmts )*
            }

            fn housekeep(&self) {
                #( #housekeep_stmts )*
            }

            fn needs_housekeep(&self) -> bool {
                false #( #needs_housekeep_terms )*
            }
        }

        impl #impl_generics #metered::MetricTreeMeta for #ident #ty_generics #where_clause {
            #meta_help
            #meta_unit
        }
        };
    }
    .into()
}

/// The names of every generic parameter on the deriving struct: type and
/// const parameter idents, plus lifetime idents without their tick.
fn generic_param_names(generics: &syn::Generics) -> HashSet<String> {
    generics
        .params
        .iter()
        .map(|param| match param {
            syn::GenericParam::Type(ty) => ty.ident.to_string(),
            syn::GenericParam::Const(konst) => konst.ident.to_string(),
            syn::GenericParam::Lifetime(lifetime) => lifetime.lifetime.ident.to_string(),
        })
        .collect()
}

/// Whether any identifier in `tokens` names one of the struct's generic
/// parameters. Lifetimes tokenize as a `'` punct followed by an ident, so
/// they are caught by the same ident comparison. Purely textual: a path
/// segment that merely shadows a parameter name is a false positive, which
/// costs the view cache for that field but stays sound.
fn tokens_mention_params(tokens: proc_macro2::TokenStream, params: &HashSet<String>) -> bool {
    tokens.into_iter().any(|tt| match tt {
        TokenTree::Group(group) => tokens_mention_params(group.stream(), params),
        TokenTree::Ident(ident) => params.contains(&ident.to_string()),
        TokenTree::Punct(_) | TokenTree::Literal(_) => false,
    })
}

/// The emitted name shape for a `#[derive(MetricTree)]` field.
#[derive(Debug)]
enum FieldShape {
    /// Emit the field under this segment (the field name, or a `rename` value).
    Segment(String),
    /// Drop the field's segment so its children sit at the parent level.
    Flatten,
}

/// How a `#[derive(MetricTree)]` field is exposed.
#[derive(Clone, Copy, Debug)]
enum FieldKind {
    /// Expose through the `MetricsView` trait.
    View,
    /// Force a gauge by wrapping in `AsGauge`.
    Gauge,
    /// Force a counter by wrapping in `AsCounter`.
    Counter,
    /// Force an info leaf by wrapping in `AsInfo`.
    Info,
    /// Expose through the field's own `MetricTree` impl.
    Tree,
}

/// The full per-field plan parsed from its `#[metric(...)]` attribute.
#[derive(Debug)]
struct FieldPlan {
    shape: FieldShape,
    kind: FieldKind,
    help: Option<String>,
    unit: Option<String>,
}

/// Whether `ty` (behind any references) is textually one of the unsigned
/// atomics whose default exposition kinds disagree (`AtomicU64` -> counter,
/// `AtomicUsize` -> gauge). A proc macro cannot resolve types, so this is a
/// last-segment name check -- it catches the standard-library spellings.
fn is_ambiguous_atomic(ty: &Type) -> bool {
    let target = match ty {
        Type::Reference(reference) => reference.elem.as_ref(),
        other => other,
    };
    match target {
        Type::Path(path) => {
            path.path.segments.last().is_some_and(|segment| {
                segment.ident == "AtomicU64" || segment.ident == "AtomicUsize"
            })
        }
        _ => false,
    }
}

/// Parses the optional `#[metric(...)]` attribute on a field into a [`FieldPlan`].
fn field_plan(field: &Field) -> syn::Result<Option<FieldPlan>> {
    let field_ident = field.ident.as_ref().expect("named field has an ident");
    let attr = crate::metric_attr::parse(&field.attrs)?;
    if !attr.include {
        return Ok(None);
    }

    if let (Some(flatten_span), Some(_)) = (attr.flatten, &attr.rename) {
        return Err(syn::Error::new(
            flatten_span,
            "`#[metric(flatten)]` and `#[metric(rename = ...)]` cannot be combined",
        ));
    }
    let explicit_kinds: Vec<_> = [attr.gauge, attr.counter, attr.info, attr.tree]
        .into_iter()
        .flatten()
        .collect();
    if explicit_kinds.len() > 1 {
        return Err(syn::Error::new(
            explicit_kinds[1],
            "`#[metric(gauge)]`, `counter`, `info`, and `tree` are mutually exclusive",
        ));
    }
    if let Some(flatten_span) = attr.flatten {
        if attr.gauge.or(attr.counter).or(attr.info).is_some() {
            return Err(syn::Error::new(
                flatten_span,
                "`#[metrics(flatten)]` cannot be combined with `gauge`, `counter`, or `info`",
            ));
        }
    }

    let shape = if attr.flatten.is_some() {
        FieldShape::Flatten
    } else {
        FieldShape::Segment(
            attr.rename
                .map(|(segment, _)| segment)
                .unwrap_or_else(|| field_ident.to_string()),
        )
    };
    let kind = if attr.flatten.is_some() {
        FieldKind::Tree
    } else if attr.gauge.is_some() {
        FieldKind::Gauge
    } else if attr.counter.is_some() {
        FieldKind::Counter
    } else if attr.info.is_some() {
        FieldKind::Info
    } else if attr.tree.is_some() || attr.singular.is_some() {
        // A bare `#[metric]` on an unsigned atomic would silently pick up the
        // ambient Metric impl's exposition kind -- and AtomicU64 (counter) vs
        // AtomicUsize (gauge) disagree, so the wire type would hinge on which
        // integer width the field happens to use. Require the kind spelled out.
        if attr.tree.is_none() && is_ambiguous_atomic(&field.ty) {
            let span = attr.singular.unwrap_or_else(|| field_ident.span());
            return Err(syn::Error::new(
                span,
                "unsigned atomics are ambiguous under a bare `#[metric]`: \
                 declare the exposition kind with `#[metric(counter)]` or \
                 `#[metric(gauge)]`",
            ));
        }
        FieldKind::Tree
    } else {
        FieldKind::View
    };

    // A component field mounts a whole `MetricsView`, not one family, so there
    // is no single series for the metadata to attach to; silently dropping it
    // (the previous behavior) would hide typos forever.
    if matches!(kind, FieldKind::View) {
        if let Some((_, span)) = attr.help.as_ref().or(attr.unit.as_ref()) {
            return Err(syn::Error::new(
                *span,
                "`help`/`unit` are not supported on component fields — set them \
                 on the entries inside the view",
            ));
        }
    }

    Ok(Some(FieldPlan {
        shape,
        kind,
        help: attr.help.map(|(help, _)| help),
        unit: attr.unit.map(|(unit, _)| unit),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_quote;

    /// The first field of `input`'s struct body.
    fn first_field(input: DeriveInput) -> Field {
        match input.data {
            Data::Struct(data) => data.fields.into_iter().next().expect("struct has a field"),
            _ => panic!("expected a struct"),
        }
    }

    #[test]
    fn help_on_a_view_field_is_rejected() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metrics(help = "rpc metrics")]
                rpc: Rpc,
            }
        };
        let err = field_plan(&first_field(input))
            .expect_err("`help` on a component field must not be silently dropped");
        assert!(
            err.to_string()
                .contains("`help`/`unit` are not supported on component fields"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn unit_on_a_view_field_is_rejected() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metrics(unit = "seconds")]
                rpc: Rpc,
            }
        };
        let err = field_plan(&first_field(input))
            .expect_err("`unit` on a component field must not be silently dropped");
        assert!(
            err.to_string()
                .contains("`help`/`unit` are not supported on component fields"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn help_on_a_leaf_field_is_kept() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metric(help = "request count")]
                requests: Counter,
            }
        };
        let plan = field_plan(&first_field(input))
            .expect("`help` is valid on a metric-family leaf")
            .expect("the field is included");
        assert_eq!(plan.help.as_deref(), Some("request count"));
    }

    #[test]
    fn flatten_and_rename_cannot_be_combined() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metrics(flatten, rename = "other")]
                inner: Inner,
            }
        };
        let err = field_plan(&first_field(input)).expect_err("flatten+rename must be rejected");
        assert!(
            err.to_string().contains("cannot be combined"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn explicit_kinds_are_mutually_exclusive() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metric(gauge, counter)]
                value: AtomicU64,
            }
        };
        let err = field_plan(&first_field(input)).expect_err("two kinds must be rejected");
        assert!(
            err.to_string().contains("mutually exclusive"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn generic_param_mentions_gate_the_view_cache() {
        let params: HashSet<String> = [String::from("T"), String::from("a")].into();

        let concrete: Type = parse_quote! { rpc::RpcMetrics };
        assert!(!tokens_mention_params(concrete.to_token_stream(), &params));

        let direct: Type = parse_quote! { T };
        assert!(tokens_mention_params(direct.to_token_stream(), &params));

        let nested: Type = parse_quote! { Wrapper<Inner<T>> };
        assert!(tokens_mention_params(nested.to_token_stream(), &params));

        let lifetime: Type = parse_quote! { Component<'a> };
        assert!(tokens_mention_params(lifetime.to_token_stream(), &params));

        let other_lifetime: Type = parse_quote! { Component<'static> };
        assert!(!tokens_mention_params(
            other_lifetime.to_token_stream(),
            &params
        ));
    }

    #[test]
    fn textual_string_shapes_are_lent_and_everything_else_renders_through_to_string() {
        let lent: [Type; 6] = [
            parse_quote! { String },
            parse_quote! { std::string::String },
            parse_quote! { &str },
            parse_quote! { &'static str },
            parse_quote! { Arc<str> },
            parse_quote! { Cow<'a, str> },
        ];
        for ty in &lent {
            assert!(
                lends_as_str(ty),
                "`{}` should lend its stored string",
                ty.to_token_stream()
            );
        }

        // Conservative: anything unrecognized keeps the ToString path.
        let rendered: [Type; 5] = [
            parse_quote! { u16 },
            parse_quote! { MyString },
            parse_quote! { Arc<String> },
            parse_quote! { Cow<'a, [u8]> },
            parse_quote! { Vec<String> },
        ];
        for ty in &rendered {
            assert!(
                !lends_as_str(ty),
                "`{}` must keep the ToString path",
                ty.to_token_stream()
            );
        }
    }

    #[test]
    fn bare_metric_on_an_ambiguous_atomic_is_rejected() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metric]
                value: AtomicU64,
            }
        };
        let err = field_plan(&first_field(input)).expect_err("ambiguous atomic must be rejected");
        assert!(
            err.to_string().contains("unsigned atomics are ambiguous"),
            "unexpected message: {err}"
        );
    }
}
