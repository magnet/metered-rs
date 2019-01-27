use std::convert::AsRef;
use syn::parse::{Parse, ParseStream};
use syn::Result;

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

pub struct MeasureRequestAttribute {
    pub paren_token: syn::token::Paren,
    pub inner: MeasureRequestAttributeInner,
}

impl MeasureRequestAttribute {
    pub fn to_requests(&self) -> Vec<MeasureRequest<'_>> {
        self.inner.to_requests()
    }
}

impl Parse for MeasureRequestAttribute {
    fn parse(input: ParseStream) -> Result<Self> {
        let content;
        let this = MeasureRequestAttribute {
            paren_token: parenthesized!(content in input),
            inner: content.parse()?,
        };

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
    fn parse(input: ParseStream) -> Result<Self> {
        input
            .try_parse_as(MeasureRequestAttributeInner::TypePath)
            .or_else(|_| input.try_parse_as(MeasureRequestAttributeInner::KeyVal))
            .map_err(|_| {
                let err = format!(
                    "invalid format for measure attribute: {}",
                    input.to_string()
                );
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
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(MeasureRequestTypePathAttribute {
            type_paths: input.parse()?,
        })
    }
}

pub struct MeasureRequestKeyValAttribute {
    pub values: syn::punctuated::Punctuated<MeasureOptions, Token![,]>,
}

impl MeasureRequestKeyValAttribute {
    fn validate(&self, input: ParseStream) -> Result<()> {
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
                debug: None,
            })
        }
        v
    }
}

fn make_field_name(type_path: &syn::TypePath) -> String {
    use heck::SnakeCase;
    type_path
        .path
        .segments
        .last()
        .unwrap()
        .value()
        .ident
        .to_string()
        .to_snake_case()
}

impl Parse for MeasureRequestKeyValAttribute {
    fn parse(input: ParseStream) -> Result<Self> {
        let this = MeasureRequestKeyValAttribute {
            values: input.parse_terminated(MeasureOptions::parse)?,
        };

        this.validate(input)?;

        Ok(this)
    }
}

pub struct MeasureOption<K: Parse + AsRef<str>, V: Parse> {
    pub key: K,
    pub colon_token: Option<syn::Token![:]>,
    pub eq_token: syn::Token![=],
    pub value: V,
}

impl<K: Parse + AsRef<str>, V: Parse> Parse for MeasureOption<K, V> {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(MeasureOption {
            key: input.parse()?,
            colon_token: input.parse()?,
            eq_token: input.parse()?,
            value: input.parse()?,
        })
    }
}

pub type MeasureTypeOption = MeasureOption<TypeKW, MultipleVal<syn::TypePath>>;
pub type MeasureDebugOption = MeasureOption<DebugKW, InvokePath>;

pub struct TypeKW {
    pub type_token: syn::Token![type],
}

impl AsRef<str> for TypeKW {
    fn as_ref(&self) -> &str {
        "type"
    }
}

impl Parse for TypeKW {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(TypeKW {
            type_token: input.parse()?,
        })
    }
}

pub enum MultipleVal<T: Parse> {
    Single(T),
    Multiple(MultipleValArray<T>),
}

impl<T: Parse> MultipleVal<T> {
    pub fn iter(&self) -> Vec<&T> {
        match self {
            MultipleVal::Single(val) => vec![val],
            MultipleVal::Multiple(arr) => arr.values.iter().collect(),
        }
    }
}

impl<T: Parse> Parse for MultipleVal<T> {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(input
            .try_parse_as(MultipleVal::Single)
            .or_else(|_| input.parse_as(MultipleVal::Multiple))?)
    }
}

pub struct MultipleValArray<T: Parse> {
    pub bracket_token: syn::token::Bracket,
    pub values: syn::punctuated::Punctuated<T, Token![,]>,
}

impl<T: Parse> Parse for MultipleValArray<T> {
    fn parse(input: ParseStream) -> Result<Self> {
        let content;
        Ok(MultipleValArray {
            bracket_token: bracketed!(content in input),
            values: content.parse_terminated(T::parse)?,
        })
    }
}

pub struct DebugKW {
    pub ident: syn::Ident,
}

impl AsRef<str> for DebugKW {
    fn as_ref(&self) -> &str {
        "debug"
    }
}

impl Parse for DebugKW {
    fn parse(input: ParseStream) -> Result<Self> {
        let fork = input.fork();
        let ident = fork.parse::<syn::Ident>()?;
        if ident == "debug" {
            let _ = input.parse::<syn::Ident>();
            Ok(DebugKW { ident })
        } else {
            Err(input.error("Not Debug"))
        }
    }
}

pub struct InvokePath {
    pub path: syn::Path,
    pub bang: Option<syn::Token![!]>,
}

impl Parse for InvokePath {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(InvokePath {
            path: input.parse()?,
            bang: input.parse()?,
        })
    }
}

pub enum MeasureOptions {
    Type(MeasureTypeOption),
    Debug(MeasureDebugOption),
}

impl MeasureOptions {
    pub fn as_str(&self) -> &str {
        match self {
            MeasureOptions::Type(opt) => opt.key.as_ref(),
            MeasureOptions::Debug(opt) => opt.key.as_ref(),
        }
    }
}

impl Parse for MeasureOptions {
    fn parse(input: ParseStream) -> Result<Self> {
        input
            .try_parse_as(MeasureOptions::Type)
            .or_else(|_| input.try_parse_as(MeasureOptions::Debug))
            .map_err(|_| {
                let err = format!("invalid measure option: {}", input.to_string());
                input.error(err)
            })
    }
}

trait ParseStreamExt {
    fn try_parse<T: syn::parse::Parse>(&self) -> Result<T>;

    fn try_parse_as<T, R, F>(&self, f: F) -> Result<R>
    where
        T: Parse,
        F: FnOnce(T) -> R,
    {
        self.try_parse::<T>().map(f)
    }

    fn parse_as<T, R, F>(&self, f: F) -> Result<R>
    where
        T: Parse,
        F: FnOnce(T) -> R;
}

impl<'a> ParseStreamExt for ParseStream<'a> {
    // Advance the stream only if parsing was successful
    // This is probably a bad way to do it, revisit later
    // using the step API?
    fn try_parse<T: syn::parse::Parse>(&self) -> Result<T> {
        let fork = self.fork();
        fork.parse::<T>()?;
        self.parse::<T>()
    }

    fn parse_as<T, R, F>(&self, f: F) -> Result<R>
    where
        T: Parse,
        F: FnOnce(T) -> R,
    {
        self.parse::<T>().map(f)
    }
}
