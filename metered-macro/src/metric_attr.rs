//! Shared parsing for the `#[metric(...)]` / `#[metrics(...)]` helper
//! attributes that drive `#[derive(MetricTree)]` container and field shaping.

use proc_macro2::Span;
use syn::{Attribute, Lit, Meta, NestedMeta, spanned::Spanned};

/// The options recognized on a field-level `#[metric(...)]` attribute.
///
/// Each present option carries the [`Span`] of its occurrence so callers can
/// point combination errors at the offending option rather than at the field.
#[derive(Debug, Default)]
pub(crate) struct MetricAttr {
    /// Whether a `#[metric]` attribute was present.
    pub include: bool,
    /// The singular `#[metric]` attribute (a metric-family leaf), if present.
    pub singular: Option<Span>,
    /// `#[metrics(flatten)]` -- drop this node's name segment.
    pub flatten: Option<Span>,
    /// `#[metrics(rename = "...")]` / `#[metric(rename = "...")]` -- override this node's name segment.
    pub rename: Option<(String, Span)>,
    /// `#[metric(gauge)]` -- expose this leaf field as a gauge.
    pub gauge: Option<Span>,
    /// `#[metric(counter)]` -- expose this leaf field as a counter.
    pub counter: Option<Span>,
    /// `#[metrics(info)]` -- expose this field as an info metric.
    pub info: Option<Span>,
    /// `#[metrics(tree)]` -- expose this field through its MetricTree.
    pub tree: Option<Span>,
    /// `#[metrics(help = "...")]` / `#[metric(help = "...")]` -- HELP text for this field's family.
    pub help: Option<(String, Span)>,
    /// `#[metrics(unit = "...")]` / `#[metric(unit = "...")]` -- UNIT metadata for this field's family.
    pub unit: Option<(String, Span)>,
}

/// Records a flag option's span, rejecting a repeated occurrence.
fn set_flag(slot: &mut Option<Span>, span: Span, option: &str) -> syn::Result<()> {
    if slot.is_some() {
        return Err(syn::Error::new(
            span,
            format!("`{option}` option is defined more than once"),
        ));
    }
    *slot = Some(span);
    Ok(())
}

/// Records a valued option, rejecting a repeated occurrence.
fn set_value(
    slot: &mut Option<(String, Span)>,
    value: String,
    span: Span,
    option: &str,
) -> syn::Result<()> {
    if slot.is_some() {
        return Err(syn::Error::new(
            span,
            format!("`{option}` option is defined more than once"),
        ));
    }
    *slot = Some((value, span));
    Ok(())
}

/// Parses every field-level `#[metric(...)]` attribute in `attrs` into one
/// [`MetricAttr`].
///
/// Accepts bare `#[metrics]` (component/subtree default), bare `#[metric]`
/// (metric-family leaf default), and options such as `flatten`, `info`, `gauge`,
/// `counter`, `rename`, `help`, and `unit`. Whether a given site allows a
/// particular option is the caller's concern.
pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<MetricAttr> {
    let mut parsed = MetricAttr::default();
    for attr in attrs
        .iter()
        .filter(|attr| attr.path.is_ident("metric") || attr.path.is_ident("metrics"))
    {
        parsed.include = true;
        let attr_name = if attr.path.is_ident("metric") {
            if parsed.singular.is_none() {
                parsed.singular = Some(attr.span());
            }
            "metric"
        } else {
            "metrics"
        };
        let list = match attr.parse_meta()? {
            Meta::List(list) => list,
            Meta::Path(_) => continue,
            _ => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "expected `#[metric(...)]` or `#[metrics(...)]`",
                ));
            }
        };
        for nested in list.nested {
            match nested {
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("flatten") => {
                    set_flag(&mut parsed.flatten, path.span(), "flatten")?;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("gauge") => {
                    set_flag(&mut parsed.gauge, path.span(), "gauge")?;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("counter") => {
                    set_flag(&mut parsed.counter, path.span(), "counter")?;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("info") => {
                    set_flag(&mut parsed.info, path.span(), "info")?;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("tree") => {
                    set_flag(&mut parsed.tree, path.span(), "tree")?;
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("rename") => {
                    let value = string_lit(&nv.lit, "rename")?;
                    set_value(&mut parsed.rename, value, nv.span(), "rename")?;
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("help") => {
                    let value = string_lit(&nv.lit, "help")?;
                    set_value(&mut parsed.help, value, nv.span(), "help")?;
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("unit") => {
                    let value = string_lit(&nv.lit, "unit")?;
                    set_value(&mut parsed.unit, value, nv.span(), "unit")?;
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        format!(
                            "unknown `{attr_name}` option; expected `flatten`, `rename = \"...\"`, \
                             `gauge`, `counter`, `info`, `tree`, `help = \"...\"`, or `unit = \"...\"`"
                        ),
                    ));
                }
            }
        }
    }
    Ok(parsed)
}

/// Extracts a string literal value or returns a descriptive error.
fn string_lit(lit: &Lit, option: &str) -> syn::Result<String> {
    match lit {
        Lit::Str(value) => Ok(value.value()),
        other => Err(syn::Error::new_spanned(
            other,
            format!("`{option}` must be a string literal"),
        )),
    }
}

/// Container-level `#[metrics(...)]` options on a `#[derive(MetricTree)]`
/// struct (and the `crate` path override shared with `#[derive(LabelSet)]`).
#[derive(Default)]
pub(crate) struct ContainerAttr {
    /// `#[metrics(prefix = "...")]` -- a name segment applied to the whole tree.
    pub prefix: Option<String>,
    /// `#[metrics(help = "...")]` -- HELP text associated with the whole tree.
    pub help: Option<String>,
    /// `#[metrics(unit = "...")]` -- UNIT metadata associated with the whole tree.
    pub unit: Option<String>,
    /// `#[metrics(label(k = "v", ...))]` -- constant labels on every family.
    pub labels: Vec<(String, String)>,
    /// `#[metrics(crate = "...")]` -- path the generated code uses to reach the
    /// metered runtime (default `::metered`). Shared by `MetricTree` and
    /// `LabelSet`; mirrors `#[serde(crate = "...")]` / `#[span(crate = "...")]`.
    pub krate: Option<syn::Path>,
}

impl std::fmt::Debug for ContainerAttr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContainerAttr")
            .field("prefix", &self.prefix)
            .field("help", &self.help)
            .field("unit", &self.unit)
            .field("labels", &self.labels)
            .field("krate", &self.krate.as_ref().map(|_| "<path>"))
            .finish()
    }
}

/// Parses the struct-level `#[metrics(...)]` attribute(s) into a [`ContainerAttr`].
///
/// Accepts `prefix = "<str>"`, `help = "<str>"`, `unit = "<str>"`,
/// `label(key = "value", ...)`, and `crate = "<path>"`.
pub(crate) fn parse_container(attrs: &[Attribute]) -> syn::Result<ContainerAttr> {
    let mut parsed = ContainerAttr::default();
    for attr in attrs.iter().filter(|attr| attr.path.is_ident("metrics")) {
        let list = match attr.parse_meta()? {
            Meta::List(list) => list,
            _ => return Err(syn::Error::new_spanned(attr, "expected `#[metrics(...)]`")),
        };
        for nested in list.nested {
            match nested {
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("prefix") => match nv.lit
                {
                    Lit::Str(value) => parsed.prefix = Some(value.value()),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "`prefix` must be a string literal",
                        ));
                    }
                },
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("help") => match nv.lit {
                    Lit::Str(value) => parsed.help = Some(value.value()),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "`help` must be a string literal",
                        ));
                    }
                },
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("unit") => match nv.lit {
                    Lit::Str(value) => parsed.unit = Some(value.value()),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "`unit` must be a string literal",
                        ));
                    }
                },
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("crate") => {
                    let lit = match &nv.lit {
                        Lit::Str(value) => value,
                        other => {
                            return Err(syn::Error::new_spanned(
                                other,
                                "`crate` must be a string literal path, e.g. `\"::metered\"`",
                            ));
                        }
                    };
                    if parsed.krate.is_some() {
                        return Err(syn::Error::new_spanned(
                            &nv,
                            "`crate` option is defined more than once",
                        ));
                    }
                    parsed.krate = Some(lit.parse::<syn::Path>().map_err(|_| {
                        syn::Error::new_spanned(
                            lit,
                            "`crate` must be a path, e.g. `::metered` or `crate::facade`",
                        )
                    })?);
                }
                NestedMeta::Meta(Meta::List(labels)) if labels.path.is_ident("label") => {
                    for label in labels.nested {
                        match label {
                            NestedMeta::Meta(Meta::NameValue(nv)) => {
                                let key = nv.path.get_ident().map(|i| i.to_string()).ok_or_else(
                                    || {
                                        syn::Error::new_spanned(
                                            &nv.path,
                                            "label key must be an identifier",
                                        )
                                    },
                                )?;
                                match nv.lit {
                                    Lit::Str(value) => parsed.labels.push((key, value.value())),
                                    other => {
                                        return Err(syn::Error::new_spanned(
                                            other,
                                            "label value must be a string literal",
                                        ));
                                    }
                                }
                            }
                            other => {
                                return Err(syn::Error::new_spanned(
                                    other,
                                    "expected `key = \"value\"` inside `label(...)`",
                                ));
                            }
                        }
                    }
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "unknown `metrics` option; expected `prefix = \"...\"`, `help = \"...\"`, \
                         `unit = \"...\"`, `label(...)`, or `crate = \"...\"`",
                    ));
                }
            }
        }
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::{DeriveInput, parse_quote};

    /// The attributes of the first field of `input`'s struct body.
    fn first_field_attrs(input: DeriveInput) -> Vec<Attribute> {
        match input.data {
            syn::Data::Struct(data) => {
                data.fields
                    .into_iter()
                    .next()
                    .expect("struct has a field")
                    .attrs
            }
            _ => panic!("expected a struct"),
        }
    }

    #[test]
    fn removed_view_spelling_is_an_unknown_option() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metrics(view)]
                rpc: Rpc,
            }
        };
        let err = parse(&first_field_attrs(input)).expect_err("`view` is no longer an option");
        let message = err.to_string();
        assert!(
            message.contains("unknown `metrics` option") && message.contains("`tree`"),
            "unknown-option error must name the attribute and list the real \
             options: {message}"
        );
    }

    #[test]
    fn unknown_metric_option_error_names_the_metric_attribute() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metric(bogus)]
                count: Counter,
            }
        };
        let err = parse(&first_field_attrs(input)).expect_err("`bogus` is not an option");
        assert!(
            err.to_string().contains("unknown `metric` option"),
            "error must name the attribute actually used: {err}"
        );
    }

    #[test]
    fn duplicate_rename_across_attributes_is_rejected() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metric(rename = "a")]
                #[metrics(rename = "b")]
                count: Counter,
            }
        };
        let err = parse(&first_field_attrs(input)).expect_err("last-wins rename must be rejected");
        assert!(
            err.to_string()
                .contains("`rename` option is defined more than once"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn duplicate_help_within_one_attribute_is_rejected() {
        let input: DeriveInput = parse_quote! {
            struct Metrics {
                #[metric(help = "a", help = "b")]
                count: Counter,
            }
        };
        let err = parse(&first_field_attrs(input)).expect_err("repeated help must be rejected");
        assert!(
            err.to_string()
                .contains("`help` option is defined more than once"),
            "unexpected message: {err}"
        );
    }

    #[test]
    fn unknown_container_option_error_names_the_metrics_attribute() {
        let input: DeriveInput = parse_quote! {
            #[metrics(bogus = "x")]
            struct Metrics {}
        };
        let err = match parse_container(&input.attrs) {
            Err(err) => err,
            Ok(_) => panic!("`bogus` is not a container option"),
        };
        assert!(
            err.to_string().contains("unknown `metrics` option"),
            "container error must name `metrics`, the attribute it parses: {err}"
        );
    }

    #[test]
    fn container_crate_path_is_parsed() {
        let input: DeriveInput = parse_quote! {
            #[metrics(crate = "crate::facade")]
            struct Metrics {}
        };
        let parsed = parse_container(&input.attrs).expect("crate path parses");
        let path = parsed.krate.expect("crate set");
        assert_eq!(quote::quote!(#path).to_string(), "crate :: facade");
    }
}
