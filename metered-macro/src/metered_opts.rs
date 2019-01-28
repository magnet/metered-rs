use syn::parse::{Parse, ParseStream};
use syn::Result;

use crate::attrs_common::*;

pub struct Metered<'a> {
    pub registry: &'a syn::Ident,
    pub registry_name: String,
}

pub struct MeteredKeyValAttribute {
    pub values: syn::punctuated::Punctuated<MeteredOption, Token![,]>,
}

impl MeteredKeyValAttribute {
    fn validate(&self, input: ParseStream) -> Result<()> {
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
        let registry = self
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

        let registry_name = registry.to_string();

        Metered {
            registry,
            registry_name,
        }
    }
}

impl Parse for MeteredKeyValAttribute {
    fn parse(input: ParseStream) -> Result<Self> {
        let this = MeteredKeyValAttribute {
            values: input.parse_terminated(MeteredOption::parse)?,
        };

        this.validate(input)?;

        Ok(this)
    }
}

custom_keyword!(RegistryKW, registry);

pub type MeteredRegistryOption = KVOption<RegistryKW, syn::Ident>;

pub enum MeteredOption {
    Registry(MeteredRegistryOption),
    _Unused,
}

impl MeteredOption {
    pub fn as_str(&self) -> &str {
        match self {
            MeteredOption::Registry(opt) => opt.key.as_ref(),
            MeteredOption::_Unused => "unused",
        }
    }
}

impl Parse for MeteredOption {
    fn parse(input: ParseStream) -> Result<Self> {
        input.try_parse_as(MeteredOption::Registry).map_err(|_| {
            let err = format!("invalid metered option: {}", input.to_string());
            input.error(err)
        })
    }
}
