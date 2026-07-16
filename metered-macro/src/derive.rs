//! `#[derive(LabelSet)]` and `#[derive(MetricTree)]`.

use proc_macro::TokenStream;
use quote::quote;
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
            ))
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

/// Derives [`metered::LabelSet`]: each field becomes a label whose name is the
/// field name and whose value is `field.to_string()`.
pub fn label_set(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse(input) {
        Ok(input) => input,
        Err(e) => return e.to_compile_error().into(),
    };
    let fields = match named_fields(&input, "LabelSet") {
        Ok(fields) => fields,
        Err(err) => return err,
    };
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let field_names: Vec<_> = fields
        .iter()
        .map(|field| field.ident.as_ref().expect("named field has an ident"))
        .collect();

    quote! {
        impl #impl_generics ::metered::LabelSet for #ident #ty_generics #where_clause {
            fn encode_labels(&self, out: &mut ::std::vec::Vec<(::std::string::String, ::std::string::String)>) {
                #(
                    out.push((
                        ::std::string::String::from(stringify!(#field_names)),
                        ::std::string::ToString::to_string(&self.#field_names),
                    ));
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
    // A `#[metric(prefix = ...)]` joins a segment onto the inherited name; a
    // `#[metric(label(...))]` appends constant labels. Both compose when the
    // tree is nested, and let a root tree carry its own prefix/labels without a
    // hand-wired `Registry`.
    let prefix_apply = match &container.prefix {
        Some(prefix) => quote! {
            let name: &str = &::metered::join_name(name, #prefix);
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
        quote! {
            let labels: &[(&str, &str)] = &{
                let mut all = labels.to_vec();
                all.extend([ #(#pairs),* ]);
                all
            };
        }
    };
    let meta_help = match &container.help {
        Some(help) => quote! {
            fn help() -> ::std::option::Option<::metered::Help> {
                ::std::option::Option::Some(::metered::Help::from(#help))
            }
        },
        None => quote! {},
    };
    let meta_unit = match &container.unit {
        Some(unit) => quote! {
            fn unit() -> ::std::option::Option<::metered::Unit> {
                ::std::option::Option::Some(::metered::Unit::from(#unit))
            }
        },
        None => quote! {},
    };

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
            Err(err) => return err,
        };
        let Some(plan) = plan else {
            continue;
        };

        // The family name this field contributes, so per-field `help`/`unit`
        // attach to the exact series. A flattened field emits under the
        // inherited `name`; a segment joins onto it.
        let family_name = match &plan.shape {
            FieldShape::Flatten => quote! { name },
            FieldShape::Segment(segment) => quote! { &::metered::join_name(name, #segment) },
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
            FieldKind::Tree | FieldKind::Gauge | FieldKind::Counter => {
                // `gauge`/`counter` force the leaf's OpenMetrics type by wrapping
                // the field in `AsGauge`/`AsCounter`; without them the field's
                // own `Metric` impl decides (e.g. `AtomicU64` defaults to a
                // counter).
                let node_inner = match plan.kind {
                    FieldKind::Tree => quote! { #field_value },
                    FieldKind::Gauge => quote! { &::metered::AsGauge::from(#field_value) },
                    FieldKind::Counter => quote! { &::metered::AsCounter::from(#field_value) },
                    FieldKind::Info | FieldKind::View => unreachable!(),
                };
                // Wrap the field so its name segment is shaped (renamed, or
                // flattened away). The same shaped node drives both describe and
                // collect.
                let node = match &plan.shape {
                    FieldShape::Flatten => quote! { &::metered::Flatten::new(#node_inner) },
                    FieldShape::Segment(segment) => {
                        quote! { &::metered::Renamed::new(#segment, #node_inner) }
                    }
                };

                describe_stmts.push(quote! {
                    #help_stmt
                    #unit_stmt
                    ::metered::MetricTree::describe(#node, name, labels, schema);
                });
                collect_stmts.push(quote! {
                    ::metered::MetricTree::collect(#node, name, labels, values);
                });
                // Housekeeping ignores name shaping and leaf-type wrapping, so
                // forward straight to the field.
                housekeep_stmts.push(quote! {
                    ::metered::MetricTree::housekeep(#field_value);
                });
                needs_housekeep_terms.push(quote! {
                    || ::metered::MetricTree::needs_housekeep(#field_value)
                });
            }
            FieldKind::Info => {
                describe_stmts.push(quote! {
                    #help_stmt
                    #unit_stmt
                    let __info_labels = ::metered::Info::labels(#field_value);
                    let __all_labels: ::std::vec::Vec<(&str, &str)> = labels
                        .iter()
                        .copied()
                        .chain(__info_labels.as_slice().iter().map(|(key, value)| (key.as_str(), value.as_str())))
                        .collect();
                    schema.add_family(#family_name, ::metered::MetricType::Info, &__all_labels);
                });
                collect_stmts.push(quote! {
                    let __info_labels = ::metered::Info::labels(#field_value);
                    let __all_labels: ::std::vec::Vec<(&str, &str)> = labels
                        .iter()
                        .copied()
                        .chain(__info_labels.as_slice().iter().map(|(key, value)| (key.as_str(), value.as_str())))
                        .collect();
                    values.sample(&::std::format!("{}_info", #family_name), &__all_labels, 1u64);
                });
                housekeep_stmts.push(quote! {});
                needs_housekeep_terms.push(quote! {});
            }
            FieldKind::View => {
                let describe_stmt = match &plan.shape {
                    FieldShape::Flatten => quote! {
                        let __view = <#field_target_ty as ::metered::MetricsView>::metrics_view();
                        __view.describe_prefixed(#field_value, Some(name), labels, schema);
                    },
                    FieldShape::Segment(segment) => quote! {
                        let __view = <#field_target_ty as ::metered::MetricsView>::metrics_view();
                        let __parent = ::metered::join_name(name, #segment);
                        __view.describe_prefixed(#field_value, Some(__parent.as_str()), labels, schema);
                    },
                };
                let collect_stmt = match &plan.shape {
                    FieldShape::Flatten => quote! {
                        let __view = <#field_target_ty as ::metered::MetricsView>::metrics_view();
                        __view.collect_prefixed(#field_value, Some(name), labels, values);
                    },
                    FieldShape::Segment(segment) => quote! {
                        let __view = <#field_target_ty as ::metered::MetricsView>::metrics_view();
                        let __parent = ::metered::join_name(name, #segment);
                        __view.collect_prefixed(#field_value, Some(__parent.as_str()), labels, values);
                    },
                };
                describe_stmts.push(describe_stmt);
                collect_stmts.push(collect_stmt);
                housekeep_stmts.push(quote! {
                    let __view = <#field_target_ty as ::metered::MetricsView>::metrics_view();
                    __view.housekeep_entries(#field_value);
                });
                // Mirror the housekeep routing above: a view field must OR its
                // entries' upkeep into the tree's `needs_housekeep`, or a metric
                // that lives only behind a `#[metric(view)]` field (e.g. a
                // dynamic exponential histogram) would never be housekept and
                // would freeze once saturated.
                needs_housekeep_terms.push(quote! {
                    || {
                        let __view = <#field_target_ty as ::metered::MetricsView>::metrics_view();
                        __view.needs_housekeep_entries(#field_value)
                    }
                });
            }
        }
    }

    quote! {
        impl #impl_generics ::metered::MetricTree for #ident #ty_generics #where_clause {
            fn describe(
                &self,
                name: &str,
                labels: &[(&str, &str)],
                schema: &mut ::metered::MetricSchema,
            ) {
                #prefix_apply
                #labels_apply
                #( #describe_stmts )*
            }

            fn collect(
                &self,
                name: &str,
                labels: &[(&str, &str)],
                values: &mut ::metered::MetricValues,
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

        impl #impl_generics ::metered::MetricTreeMeta for #ident #ty_generics #where_clause {
            #meta_help
            #meta_unit
        }
    }
    .into()
}

/// The emitted name shape for a `#[derive(MetricTree)]` field.
enum FieldShape {
    /// Emit the field under this segment (the field name, or a `rename` value).
    Segment(String),
    /// Drop the field's segment so its children sit at the parent level.
    Flatten,
}

/// How a `#[derive(MetricTree)]` field is exposed.
#[derive(Clone, Copy)]
enum FieldKind {
    /// Expose through the `MetricsView` trait.
    View,
    /// Force a gauge by wrapping in `AsGauge`.
    Gauge,
    /// Force a counter by wrapping in `AsCounter`.
    Counter,
    /// Expose through the `Info` trait.
    Info,
    /// Expose through the field's own `MetricTree` impl.
    Tree,
}

/// The full per-field plan parsed from its `#[metric(...)]` attribute.
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
fn field_plan(field: &Field) -> Result<Option<FieldPlan>, TokenStream> {
    let field_ident = field.ident.as_ref().expect("named field has an ident");
    let attr = crate::metric_attr::parse(&field.attrs)
        .map_err(|e| -> TokenStream { e.to_compile_error().into() })?;
    if !attr.include {
        return Ok(None);
    }

    if attr.flatten && attr.rename.is_some() {
        return Err(error(
            field_ident,
            "`#[metric(flatten)]` and `#[metric(rename = ...)]` cannot be combined",
        ));
    }
    let explicit_kinds = [attr.gauge, attr.counter, attr.info, attr.tree, attr.view]
        .iter()
        .filter(|enabled| **enabled)
        .count();
    if explicit_kinds > 1 {
        return Err(error(
            field_ident,
            "`#[metric(gauge)]`, `counter`, `info`, `tree`, and `view` are mutually exclusive",
        ));
    }
    if attr.flatten && (attr.gauge || attr.counter || attr.info || attr.view) {
        return Err(error(
            field_ident,
            "`#[metrics(flatten)]` cannot be combined with `gauge`, `counter`, `info`, or `view`",
        ));
    }

    let shape = if attr.flatten {
        FieldShape::Flatten
    } else {
        FieldShape::Segment(attr.rename.unwrap_or_else(|| field_ident.to_string()))
    };
    let kind = if attr.flatten {
        FieldKind::Tree
    } else if attr.gauge {
        FieldKind::Gauge
    } else if attr.counter {
        FieldKind::Counter
    } else if attr.info {
        FieldKind::Info
    } else if attr.tree || attr.singular {
        // A bare `#[metric]` on an unsigned atomic would silently pick up the
        // ambient Metric impl's exposition kind -- and AtomicU64 (counter) vs
        // AtomicUsize (gauge) disagree, so the wire type would hinge on which
        // integer width the field happens to use. Require the kind spelled out.
        if !attr.tree && is_ambiguous_atomic(&field.ty) {
            return Err(error(
                field_ident,
                "unsigned atomics are ambiguous under a bare `#[metric]`: \
                 declare the exposition kind with `#[metric(counter)]` or \
                 `#[metric(gauge)]`",
            ));
        }
        FieldKind::Tree
    } else {
        FieldKind::View
    };

    Ok(Some(FieldPlan {
        shape,
        kind,
        help: attr.help,
        unit: attr.unit,
    }))
}
