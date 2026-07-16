//! Shared parsing for the `#[metric(...)]` helper attribute, used by both the
//! `#[derive(MetricTree)]` field shaping and the `#[metered]` method renaming.

use syn::{Attribute, Lit, Meta, NestedMeta};

/// The options recognized on a field-level `#[metric(...)]` attribute.
#[derive(Default)]
pub(crate) struct MetricAttr {
    /// Whether a `#[metric]` attribute was present.
    pub include: bool,
    /// Whether the field was marked with singular `#[metric]` (a metric-family leaf).
    pub singular: bool,
    /// `#[metrics(flatten)]` -- drop this node's name segment.
    pub flatten: bool,
    /// `#[metrics(rename = "...")]` / `#[metric(rename = "...")]` -- override this node's name segment.
    pub rename: Option<String>,
    /// `#[metric(gauge)]` -- expose this leaf field as a gauge.
    pub gauge: bool,
    /// `#[metric(counter)]` -- expose this leaf field as a counter.
    pub counter: bool,
    /// `#[metrics(info)]` -- expose this field as an info metric.
    pub info: bool,
    /// `#[metrics(tree)]` -- expose this field through its MetricTree.
    pub tree: bool,
    /// `#[metrics(view)]` -- deprecated spelling for the default MetricsView mount.
    pub view: bool,
    /// `#[metrics(help = "...")]` / `#[metric(help = "...")]` -- HELP text for this field's family.
    pub help: Option<String>,
    /// `#[metrics(unit = "...")]` / `#[metric(unit = "...")]` -- UNIT metadata for this field's family.
    pub unit: Option<String>,
}

/// Parses every field-level `#[metric(...)]` attribute in `attrs` into one
/// [`MetricAttr`].
///
/// Accepts bare `#[metrics]` (component/subtree default), bare `#[metric]`
/// (metric-family leaf default), and options such as `flatten`, `info`, `gauge`,
/// `counter`, `rename`, `help`, and `unit`. Whether a given site allows a
/// particular option is the caller's concern (a `#[metered]` method only uses
/// `rename`).
pub(crate) fn parse(attrs: &[Attribute]) -> syn::Result<MetricAttr> {
    let mut parsed = MetricAttr::default();
    for attr in attrs
        .iter()
        .filter(|attr| attr.path.is_ident("metric") || attr.path.is_ident("metrics"))
    {
        parsed.include = true;
        if attr.path.is_ident("metric") {
            parsed.singular = true;
        }
        let list = match attr.parse_meta()? {
            Meta::List(list) => list,
            Meta::Path(_) => continue,
            _ => {
                return Err(syn::Error::new_spanned(
                    attr,
                    "expected `#[metric(...)]` or `#[metrics(...)]`",
                ))
            }
        };
        for nested in list.nested {
            match nested {
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("flatten") => {
                    parsed.flatten = true;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("gauge") => {
                    parsed.gauge = true;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("counter") => {
                    parsed.counter = true;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("info") => {
                    parsed.info = true;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("tree") => {
                    parsed.tree = true;
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("view") => {
                    parsed.view = true;
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("rename") => {
                    parsed.rename = Some(string_lit(&nv.lit, "rename")?);
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("help") => {
                    parsed.help = Some(string_lit(&nv.lit, "help")?);
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("unit") => {
                    parsed.unit = Some(string_lit(&nv.lit, "unit")?);
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "unknown metric option; expected `flatten`, `rename = \"...\"`, \
                         `gauge`, `counter`, `info`, `tree`, `help = \"...\"`, or `unit = \"...\"`",
                    ))
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

/// Container-level `#[metrics(...)]` options on a `#[derive(MetricTree)]` struct.
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
}

/// Parses the struct-level `#[metrics(...)]` attribute(s) into a [`ContainerAttr`].
///
/// Accepts `prefix = "<str>"`, `help = "<str>"`, `unit = "<str>"`, and
/// `label(key = "value", ...)`.
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
                        ))
                    }
                },
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("help") => match nv.lit
                {
                    Lit::Str(value) => parsed.help = Some(value.value()),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "`help` must be a string literal",
                        ))
                    }
                },
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("unit") => match nv.lit
                {
                    Lit::Str(value) => parsed.unit = Some(value.value()),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "`unit` must be a string literal",
                        ))
                    }
                },
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
                                        ))
                                    }
                                }
                            }
                            other => {
                                return Err(syn::Error::new_spanned(
                                    other,
                                    "expected `key = \"value\"` inside `label(...)`",
                                ))
                            }
                        }
                    }
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "unknown `metric` option; expected `prefix = \"...\"`, `help = \"...\"`, `unit = \"...\"`, or `label(...)`",
                    ))
                }
            }
        }
    }
    Ok(parsed)
}
