//! The module supporting `#[measure]` options

use syn::{
    parse::{Parse, ParseStream},
    Result,
};

use synattra::{
    types::{extra::InvokePath, KVOption, MultipleVal},
    ParseStreamExt,
};

pub struct MeasureRequest<'a> {
    pub tpe: &'a syn::TypePath,
    pub field_name: String,
    pub debug: Option<&'a InvokePath>,
}

impl<'a> MeasureRequest<'a> {
    pub fn ident(&self) -> syn::Ident {
        syn::Ident::new(&self.field_name, proc_macro2::Span::call_site())
    }

    pub fn type_path(&self) -> &syn::TypePath {
        self.tpe
    }
}

pub enum MeasureRequestAttribute {
    Empty,
    NonEmpty(NonEmptyMeasureRequestAttribute),
}

impl MeasureRequestAttribute {
    pub fn to_requests(&self) -> Vec<MeasureRequest<'_>> {
        match self {
            MeasureRequestAttribute::Empty => Vec::new(),
            MeasureRequestAttribute::NonEmpty(req) => req.to_requests(),
        }
    }
}

impl Parse for MeasureRequestAttribute {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if input.is_empty() {
            Ok(MeasureRequestAttribute::Empty)
        } else {
            Ok(MeasureRequestAttribute::NonEmpty(
                NonEmptyMeasureRequestAttribute::parse(input)?,
            ))
        }
    }
}

pub struct NonEmptyMeasureRequestAttribute {
    pub paren_token: syn::token::Paren,
    pub inner: Option<MeasureRequestAttributeInner>,
}

impl NonEmptyMeasureRequestAttribute {
    pub fn to_requests(&self) -> Vec<MeasureRequest<'_>> {
        if let Some(ref inner) = self.inner {
            inner.to_requests()
        } else {
            Vec::new()
        }
    }
}

impl Parse for NonEmptyMeasureRequestAttribute {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let content;
        let paren_token = parenthesized!(content in input);

        let inner = if content.is_empty() {
            None
        } else {
            Some(content.parse()?)
        };

        let this = NonEmptyMeasureRequestAttribute { paren_token, inner };

        Ok(this)
    }
}

pub enum MeasureRequestAttributeInner {
    TypePath(MeasureRequestTypePathAttribute),
    KeyVal(MeasureRequestKeyValAttribute),
}

impl MeasureRequestAttributeInner {
    pub fn to_requests(&self) -> Vec<MeasureRequest<'_>> {
        match self {
            MeasureRequestAttributeInner::TypePath(type_path) => type_path.to_requests(),
            MeasureRequestAttributeInner::KeyVal(key_val) => key_val.to_requests(),
        }
    }
}

impl Parse for MeasureRequestAttributeInner {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        input
            .try_parse_as(MeasureRequestAttributeInner::TypePath)
            .or_else(|_| input.try_parse_as(MeasureRequestAttributeInner::KeyVal))
            .map_err(|_| {
                let err = format!("invalid format for measure attribute: {}", input);
                input.error(err)
            })
    }
}

pub struct MeasureRequestTypePathAttribute {
    pub type_paths: MultipleVal<syn::TypePath>,
}

impl MeasureRequestTypePathAttribute {
    pub fn to_requests(&self) -> Vec<MeasureRequest<'_>> {
        let mut v = Vec::new();
        for type_path in self.type_paths.iter() {
            let field_name = make_field_name(type_path);
            v.push(MeasureRequest {
                tpe: type_path,
                field_name,
                debug: None,
            })
        }
        v
    }
}

impl Parse for MeasureRequestTypePathAttribute {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        Ok(MeasureRequestTypePathAttribute {
            type_paths: input.parse()?,
        })
    }
}

pub struct MeasureRequestKeyValAttribute {
    pub values: syn::punctuated::Punctuated<MeasureOptions, Token![,]>,
}

impl MeasureRequestKeyValAttribute {
    fn validate(&self, input: ParseStream<'_>) -> Result<()> {
        self.values
            .iter()
            .filter_map(|opt| {
                if let MeasureOptions::Type(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .ok_or_else(|| {
                input.error(
                    "missing `type` attribute with a path to a valid metered::Metric struct.",
                )
            })?;

        let opt_types: std::collections::HashMap<_, _> = self
            .values
            .iter()
            .map(|opt| (std::mem::discriminant(opt), opt.as_str()))
            .collect();

        for (opt_type, opt_name) in opt_types.iter() {
            let count = self
                .values
                .iter()
                .filter(|&opt| std::mem::discriminant(opt) == *opt_type)
                .count();
            if count > 1 {
                let error = format!("`{}` attribute is defined more than once.", opt_name);
                return Err(input.error(error));
            }
        }

        // self.values.iter().

        Ok(())
    }

    pub fn to_requests(&self) -> Vec<MeasureRequest<'_>> {
        let type_paths = self
            .values
            .iter()
            .filter_map(|opt| {
                if let MeasureOptions::Type(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .expect("There should be a type! This error cannot happen if the structure has been validated first!");
        let debug = self
            .values
            .iter()
            .filter_map(|opt| {
                if let MeasureOptions::Debug(dbg) = opt {
                    Some(&dbg.value)
                } else {
                    None
                }
            })
            .next();

        let mut v = Vec::new();
        for type_path in type_paths.iter() {
            let field_name = make_field_name(type_path);
            v.push(MeasureRequest {
                tpe: type_path,
                field_name,
                debug,
            })
        }
        v
    }
}

fn make_field_name(type_path: &syn::TypePath) -> String {
    use heck::ToSnakeCase;
    type_path
        .path
        .segments
        .last()
        .unwrap() // never empty
        .ident
        .to_string()
        .to_snake_case()
}

impl Parse for MeasureRequestKeyValAttribute {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let this = MeasureRequestKeyValAttribute {
            values: input.parse_terminated(MeasureOptions::parse, Token![,])?,
        };

        this.validate(input)?;

        Ok(this)
    }
}

mod kw {
    syn::custom_keyword!(debug);
}

pub type MeasureTypeOption = KVOption<syn::Token![type], MultipleVal<syn::TypePath>>;
pub type MeasureDebugOption = KVOption<kw::debug, InvokePath>;

pub enum MeasureOptions {
    Type(MeasureTypeOption),
    Debug(MeasureDebugOption),
}

impl MeasureOptions {
    pub fn as_str(&self) -> &str {
        use syn::token::Token;
        match self {
            MeasureOptions::Type(_) => <syn::Token![type]>::display(),
            MeasureOptions::Debug(_) => <kw::debug>::display(),
        }
    }
}

impl Parse for MeasureOptions {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if MeasureTypeOption::peek(input) {
            Ok(input.parse_as(MeasureOptions::Type)?)
        } else if MeasureDebugOption::peek(input) {
            Ok(input.parse_as(MeasureOptions::Debug)?)
        } else {
            let err = format!("invalid measure option: {}", input);
            Err(input.error(err))
        }
    }
}
