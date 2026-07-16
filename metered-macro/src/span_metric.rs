//! The `SpanLabels` derive: declare a span's metric labels as a typed struct and
//! get (1) a [`FromSpanFields`] impl so a span-duration adapter can rebuild the
//! typed key from a closed span, (2) a call-site `@open` macro reached through
//! `metered_info_span!`, and (3) typed `record_*` functions for `on_close`
//! fields, plus `SPAN` / `HELP` consts.
//!
//! The struct field names are the OpenMetrics labels; the `#[span("otel.field")]`
//! attribute maps each to the OpenTelemetry semconv span field it reads from.
//! Because the metric label, the emitted span field, and the close-time record
//! all expand from the same declaration, they cannot drift.
//!
//! Generated code reaches `tracing` and the runtime traits through
//! `::metered_tracing` re-exports (`::metered_tracing::__rt::tracing`,
//! [`FromSpanFields`]), so a deriving crate needs no direct `tracing` dependency.
//! The `#[span(crate = "...")]` container attribute overrides that runtime
//! path (e.g. `"::metered::tracing"` for facade-only consumers), and
//! `#[span(macro_name = "...")]` renames the exported crate-root opener when
//! two same-named types would collide. Each field type must be
//! `FromFieldValue + Default` (and `Display` for eager fields).
//!
//! [`FromSpanFields`]: ../metered_tracing/trait.FromSpanFields.html

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Lit, LitStr, Meta, NestedMeta, Type};

/// A `#[span("otel.field" [, default = "..."] [, on_close])]` field declaration.
struct LabelField {
    /// The struct field ident -- becomes the OpenMetrics label name.
    ident: syn::Ident,
    /// The struct field type -- enforced at the call site / record fn.
    ty: Type,
    /// The OpenTelemetry semconv span field the label reads from.
    otel: LitStr,
    /// Value used when the span never set the field.
    default: Option<LitStr>,
    /// `on_close`: the value is not known at open, so the field starts `Empty`
    /// and is set later (before close) via a typed `record_*` fn, rather than
    /// being passed as an eager argument to the call-site opener.
    on_close: bool,
}

struct Decl {
    name: syn::Ident,
    span_name: LitStr,
    help: Option<String>,
    /// `#[span(macro_name = "...")]`: the exported call-site opener's name,
    /// when it must differ from the type's own name (two same-named types in
    /// different modules would otherwise export two colliding crate-root
    /// macros).
    macro_name: Option<syn::Ident>,
    /// `#[span(crate = "...")]`: the path the generated code uses to reach
    /// the `metered-tracing` runtime. Defaults to `::metered_tracing`.
    krate: TokenStream2,
    /// The same runtime path as spliced into the `#[macro_export]`ed opener
    /// body, where a leading `crate` segment must become `$crate`: inside a
    /// `macro_rules!` body, `crate` resolves to the *caller's* crate, while
    /// `$crate` names the deriving crate the path was written in.
    krate_macro: TokenStream2,
    fields: Vec<LabelField>,
}

pub fn span_labels(input: TokenStream) -> TokenStream {
    let input: DeriveInput = match syn::parse(input) {
        Ok(input) => input,
        Err(e) => return e.to_compile_error().into(),
    };
    let decl = match parse_decl(&input) {
        Ok(decl) => decl,
        Err(e) => return e.to_compile_error().into(),
    };
    codegen(&decl).into()
}

fn parse_decl(input: &DeriveInput) -> syn::Result<Decl> {
    let data = match &input.data {
        Data::Struct(data) => data,
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`SpanLabels` can only be derived for structs",
            ));
        }
    };
    let named = match &data.fields {
        Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`SpanLabels` requires a struct with named fields",
            ));
        }
    };

    let container = parse_container(input)?;
    let help = container.help.or_else(|| doc_string(&input.attrs));

    let mut fields = Vec::new();
    for field in named {
        let Some(label) = parse_field(field)? else {
            continue;
        };
        fields.push(label);
    }

    let (krate, krate_macro) = match container.krate {
        Some(path) => {
            let krate_macro = macro_body_path(&path);
            (quote! { #path }, krate_macro)
        }
        None => (quote! { ::metered_tracing }, quote! { ::metered_tracing }),
    };

    Ok(Decl {
        name: input.ident.clone(),
        span_name: container.span_name,
        help,
        macro_name: container.macro_name,
        krate,
        krate_macro,
        fields,
    })
}

/// Renders `path` for splicing into the `#[macro_export]`ed opener body,
/// rewriting a leading `crate` segment to `$crate`: inside a `macro_rules!`
/// body `crate` resolves to the *caller's* crate, while `$crate` names the
/// crate the macro (and so the `#[span(crate = "crate::...")]` path) was
/// defined in -- the resolution the deriving code means.
fn macro_body_path(path: &syn::Path) -> TokenStream2 {
    let starts_at_crate = path.leading_colon.is_none()
        && path
            .segments
            .first()
            .is_some_and(|segment| segment.ident == "crate");
    if !starts_at_crate {
        return quote! { #path };
    }
    let rest = path.segments.iter().skip(1);
    quote! { $crate #(::#rest)* }
}

/// The parsed container-level `#[span(...)]` options.
struct ContainerOpts {
    span_name: LitStr,
    help: Option<String>,
    macro_name: Option<syn::Ident>,
    krate: Option<syn::Path>,
}

/// Parses the container
/// `#[span(name = "...", help = "...", macro_name = "...", crate = "...")]`
/// attribute.
fn parse_container(input: &DeriveInput) -> syn::Result<ContainerOpts> {
    let attr = input
        .attrs
        .iter()
        .find(|attr| attr.path.is_ident("span"))
        .ok_or_else(|| {
            syn::Error::new_spanned(
                &input.ident,
                "`SpanLabels` needs a `#[span(name = \"<span name>\")]` attribute",
            )
        })?;

    let mut name = None;
    let mut help = None;
    let mut macro_name = None;
    let mut krate = None;
    if let Meta::List(list) = attr.parse_meta()? {
        for nested in list.nested {
            match nested {
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("name") => {
                    name = Some(as_lit_str(&nv.lit, "name")?);
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("help") => {
                    help = Some(as_lit_str(&nv.lit, "help")?.value());
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("macro_name") => {
                    let lit = as_lit_str(&nv.lit, "macro_name")?;
                    macro_name = Some(lit.parse::<syn::Ident>().map_err(|_| {
                        syn::Error::new_spanned(
                            &lit,
                            "`macro_name` must be a valid Rust identifier",
                        )
                    })?);
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("crate") => {
                    let lit = as_lit_str(&nv.lit, "crate")?;
                    krate = Some(lit.parse::<syn::Path>().map_err(|_| {
                        syn::Error::new_spanned(
                            &lit,
                            "`crate` must be a path, e.g. `::metered::tracing`",
                        )
                    })?);
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "expected `name = \"...\"`, `help = \"...\"`, \
                         `macro_name = \"...\"`, or `crate = \"...\"`",
                    ));
                }
            }
        }
    }
    let span_name =
        name.ok_or_else(|| syn::Error::new_spanned(attr, "`#[span(name = \"...\")]` is required"))?;
    Ok(ContainerOpts {
        span_name,
        help,
        macro_name,
        krate,
    })
}

/// Parses a field's `#[span("otel.field" [, default = "..."] [, on_close])]`.
fn parse_field(field: &syn::Field) -> syn::Result<Option<LabelField>> {
    let Some(attr) = field.attrs.iter().find(|attr| attr.path.is_ident("span")) else {
        return Ok(None);
    };
    let ident = field
        .ident
        .clone()
        .expect("named struct field has an ident");

    let mut otel = None;
    let mut default = None;
    let mut on_close = false;
    if let Meta::List(list) = attr.parse_meta()? {
        for nested in list.nested {
            match nested {
                NestedMeta::Lit(lit) => otel = Some(as_lit_str(&lit, "span field")?),
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("field") => {
                    otel = Some(as_lit_str(&nv.lit, "field")?)
                }
                NestedMeta::Meta(Meta::NameValue(nv)) if nv.path.is_ident("default") => {
                    default = Some(as_lit_str(&nv.lit, "default")?)
                }
                NestedMeta::Meta(Meta::Path(path)) if path.is_ident("on_close") => on_close = true,
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "expected `\"otel.field\"`, `default = \"...\"`, or `on_close`",
                    ));
                }
            }
        }
    }
    let otel = otel.ok_or_else(|| {
        syn::Error::new_spanned(attr, "`#[span(\"<otel semconv field>\")]` is required")
    })?;

    Ok(Some(LabelField {
        ident,
        ty: field.ty.clone(),
        otel,
        default,
        on_close,
    }))
}

fn as_lit_str(lit: &Lit, what: &str) -> syn::Result<LitStr> {
    match lit {
        Lit::Str(s) => Ok(s.clone()),
        other => Err(syn::Error::new_spanned(
            other,
            format!("`{what}` must be a string literal"),
        )),
    }
}

fn doc_string(attrs: &[syn::Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|attr| attr.path.is_ident("doc"))
        .filter_map(|attr| match attr.parse_meta() {
            Ok(Meta::NameValue(nv)) => match nv.lit {
                Lit::Str(s) => Some(s.value().trim().to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}

fn codegen(decl: &Decl) -> TokenStream2 {
    let Decl {
        name,
        span_name,
        help,
        macro_name,
        krate,
        krate_macro,
        fields,
    } = decl;

    // --- SPAN / HELP consts: single-source the span name and help text. ---
    let help_const = match help {
        Some(text) => quote! {
            /// The OpenMetrics `# HELP` text declared for this span's metrics.
            pub const HELP: &'static str = #text;
        },
        None => quote! {},
    };

    // --- FromSpanFields: build the typed key from a closed span's fields. ---
    // Runs in the tracing layer's `on_close` for every matched span, so it must
    // never panic. Each present field converts **typed** through
    // `FromFieldValue` (a natively-recorded integer never round-trips through a
    // string); a value that does not convert is a `SpanFieldsError`, which the
    // layer turns into a skipped, counted close rather than a defaulted label.
    // An *absent* field falls back to the declared `default` literal (whose
    // failure to convert is equally an error -- a declaration bug) or the
    // type's `Default`.
    let from_fields = fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.ty;
        let otel = &f.otel;
        let fallback = match &f.default {
            Some(default) => quote! {
                <#ty as #krate::FromFieldValue>::from_text(#default)
                    .map_err(|error| #krate::SpanFieldsError {
                        field: #otel,
                        error,
                    })?
            },
            None => quote! { <#ty as ::core::default::Default>::default() },
        };
        quote! {
            #ident: match fields.value(#otel) {
                ::core::option::Option::Some(__value) => {
                    <#ty as #krate::FromFieldValue>::from_field_value(__value)
                        .map_err(|error| #krate::SpanFieldsError {
                            field: #otel,
                            error,
                        })?
                }
                ::core::option::Option::None => #fallback,
            },
        }
    });

    // --- Typed record_* setters for `on_close` fields (single-source the name). ---
    // `tracing` is reached through `metered_tracing`'s re-export so a crate that
    // derives `SpanLabels` needs no direct `tracing` dependency of its own.
    let record_fns = fields.iter().filter(|f| f.on_close).map(|f| {
        let fn_name = format_ident!("record_{}", f.ident);
        let ty = &f.ty;
        let otel = &f.otel;
        let doc = format!("Records the `{}` label onto `span` at close.", f.ident);
        quote! {
            #[doc = #doc]
            pub fn #fn_name(span: &#krate::__rt::tracing::Span, value: #ty) {
                #krate::__rt::tracing::Span::record(
                    span,
                    #otel,
                    #krate::__rt::tracing::field::display(&value),
                );
            }
        }
    });

    // --- Call-site `@open` macro (reached via `metered_info_span!`). ---
    let eager: Vec<&LabelField> = fields.iter().filter(|f| !f.on_close).collect();
    let eager_matchers = eager.iter().map(|f| {
        let id = &f.ident;
        quote! { #id = $ #id : expr }
    });
    let eager_lets = eager.iter().map(|f| {
        let id = &f.ident;
        let ty = &f.ty;
        quote! { let #id: #ty = $ #id; }
    });
    let eager_fields = eager.iter().map(|f| {
        let id = &f.ident;
        let otel = &f.otel;
        quote! { #otel = % #id, }
    });
    let on_close_fields: Vec<TokenStream2> = fields
        .iter()
        .filter(|f| f.on_close)
        .map(|f| {
            let otel = &f.otel;
            quote! { #otel = #krate_macro::__rt::tracing::field::Empty, }
        })
        .collect();

    let (matcher, extra_expand) = if eager.is_empty() {
        (quote! { $($__extra:tt)* }, quote! { $($__extra)* })
    } else {
        (
            quote! { #(#eager_matchers),* $(, $($__extra:tt)*)? },
            quote! { $($($__extra)*)? },
        )
    };

    // The call-site opener is `#[macro_export]`ed (and so lives at the crate
    // root): that makes it reachable from any module and before its textual
    // definition -- closing the "only works in the defining module, after the
    // struct" footgun -- and usable by dependent crates. By default it shares
    // the labels type's name (the type and the macro are in different
    // namespaces); `metered_info_span!(Type; ...)` dispatches to it. Because
    // every opener lands at the crate root, two same-named types in different
    // modules would export two colliding crate-root macros -- the
    // `#[span(macro_name = "...")]` escape hatch renames one of them.
    let opener = macro_name.as_ref().unwrap_or(name);
    let emitter = quote! {
        #[macro_export]
        macro_rules! #opener {
            (@open #matcher) => {{
                #(#eager_lets)*
                #krate_macro::__rt::tracing::info_span!(
                    #span_name,
                    #(#eager_fields)*
                    #(#on_close_fields)*
                    #extra_expand
                )
            }};
        }
    };

    // The declared `#[span("...")]` names, eager and `on_close` alike: the
    // full set `try_from_span_fields` can read, declared so the tracing layer
    // captures exactly these fields and drops everything else on the span.
    let otel_names = fields.iter().map(|f| &f.otel);

    quote! {
        impl #name {
            /// The tracing span name these labels are read from.
            pub const SPAN: &'static str = #span_name;

            #help_const

            #(#record_fns)*
        }

        impl #krate::FromSpanFields for #name {
            fn try_from_span_fields(
                fields: &#krate::SpanFields,
            ) -> ::core::result::Result<Self, #krate::SpanFieldsError> {
                ::core::result::Result::Ok(#name {
                    #(#from_fields)*
                })
            }

            fn span_field_names() -> ::core::option::Option<&'static [&'static str]> {
                ::core::option::Option::Some(&[#(#otel_names),*])
            }
        }

        #emitter
    }
}
