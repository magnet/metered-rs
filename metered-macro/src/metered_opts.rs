//! The module supporting `#[metered]` options

use syn::{
    parse::{Parse, ParseStream},
    Result,
};

use synattra::{types::KVOption, *};

use std::borrow::Cow;

pub struct Metered<'a> {
    pub registry_ident: &'a syn::Ident,
    pub registry_name: String,
    pub registry_expr: Cow<'a, syn::Expr>,
    pub visibility: Cow<'a, syn::Visibility>,
}

pub struct MeteredKeyValAttribute {
    pub values: syn::punctuated::Punctuated<MeteredOption, Token![,]>,
}

impl MeteredKeyValAttribute {
    fn validate(&self, input: ParseStream<'_>) -> Result<()> {
        self.values
            .iter()
            .filter_map(|opt| {
                if let MeteredOption::Registry(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .ok_or_else(|| input.error("missing `registry` attribute."))?;

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

        Ok(())
    }

    pub fn to_metered(&self) -> Metered<'_> {
        let registry_ident = self
            .values
            .iter()
            .filter_map(|opt| {
                if let MeteredOption::Registry(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .expect("There should be a registry! This error cannot happen if the structure has been validated first!");

        let registry_name = registry_ident.to_string();

        let registry_expr = self
            .values
            .iter()
            .filter_map(|opt| {
                if let MeteredOption::RegistryExpr(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .map(Cow::Borrowed)
            .unwrap_or_else(|| Cow::Owned(syn::parse_str::<syn::Expr>("self.metrics").unwrap()));

        let visibility = self
            .values
            .iter()
            .filter_map(|opt| {
                if let MeteredOption::Visibility(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .map(Cow::Borrowed)
            .unwrap_or_else(|| {
                Cow::Owned(syn::parse_str::<syn::Visibility>("pub(crate)").unwrap())
            });
        Metered {
            registry_ident,
            registry_name,
            registry_expr,
            visibility,
        }
    }
}

impl Parse for MeteredKeyValAttribute {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let this = MeteredKeyValAttribute {
            values: input.parse_terminated(MeteredOption::parse, Token![,])?,
        };

        this.validate(input)?;

        Ok(this)
    }
}

mod kw {
    syn::custom_keyword!(registry);
    syn::custom_keyword!(registry_expr);
    syn::custom_keyword!(visibility);
}

pub type MeteredRegistryOption = KVOption<kw::registry, syn::Ident>;

pub type MeteredRegistryExprOption = KVOption<kw::registry_expr, syn::Expr>;

pub type MeteredVisibilityOption = KVOption<kw::visibility, syn::Visibility>;

#[allow(clippy::large_enum_variant)]
pub enum MeteredOption {
    Registry(MeteredRegistryOption),
    RegistryExpr(MeteredRegistryExprOption),
    Visibility(MeteredVisibilityOption),
}

impl MeteredOption {
    pub fn as_str(&self) -> &str {
        use syn::token::Token;
        match self {
            MeteredOption::Registry(_) => <kw::registry>::display(),
            MeteredOption::RegistryExpr(_) => <kw::registry_expr>::display(),
            MeteredOption::Visibility(_) => <kw::visibility>::display(),
        }
    }
}

impl Parse for MeteredOption {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if MeteredRegistryOption::peek(input) {
            Ok(input.parse_as(MeteredOption::Registry)?)
        } else if MeteredRegistryExprOption::peek(input) {
            Ok(input.parse_as(MeteredOption::RegistryExpr)?)
        } else if MeteredVisibilityOption::peek(input) {
            Ok(input.parse_as(MeteredOption::Visibility)?)
        } else {
            let err = format!("invalid metered option: {}", input);
            Err(input.error(err))
        }
    }
}
