//! The module supporting `#[error_count]` options

use syn::{
    parse::{Parse, ParseStream},
    Result,
};

use synattra::{types::KVOption, *};

use std::borrow::Cow;

pub struct ErrorCountOpts<'a> {
    pub name_ident: &'a syn::Ident,
    pub visibility: Cow<'a, syn::Visibility>,
    pub skip_cleared: bool,
}

pub struct ErrorCountKeyValAttribute {
    pub values: syn::punctuated::Punctuated<ErrorCountOption, Token![,]>,
}

impl ErrorCountKeyValAttribute {
    fn validate(&self, input: ParseStream<'_>) -> Result<()> {
        self.values
            .iter()
            .filter_map(|opt| {
                if let ErrorCountOption::Name(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .ok_or_else(|| input.error("missing `name` attribute."))?;

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

    pub fn to_error_count_opts(&self) -> ErrorCountOpts<'_> {
        let name_ident = self
            .values
            .iter()
            .filter_map(|opt| {
                if let ErrorCountOption::Name(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .expect("There should be a name! This error cannot happen if the structure has been validated first!");

        let visibility = self
            .values
            .iter()
            .filter_map(|opt| {
                if let ErrorCountOption::Visibility(tpe) = opt {
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

        let skip_cleared = self
            .values
            .iter()
            .filter_map(|opt| {
                if let ErrorCountOption::SkipCleared(tpe) = opt {
                    Some(&tpe.value)
                } else {
                    None
                }
            })
            .next()
            .map(|value| value.value)
            .unwrap_or(cfg!(feature = "error-count-skip-cleared-by-default"));

        ErrorCountOpts {
            name_ident,
            visibility,
            skip_cleared,
        }
    }
}

impl Parse for ErrorCountKeyValAttribute {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let this = ErrorCountKeyValAttribute {
            values: input.parse_terminated(ErrorCountOption::parse, Token![,])?,
        };

        this.validate(input)?;

        Ok(this)
    }
}

mod kw {
    syn::custom_keyword!(name);
    syn::custom_keyword!(visibility);
    syn::custom_keyword!(skip_cleared);
}

pub type ErrorCountNameOption = KVOption<kw::name, syn::Ident>;

pub type ErrorCountVisibilityOption = KVOption<kw::visibility, syn::Visibility>;

pub type ErrorCountSkipClearedOption = KVOption<kw::skip_cleared, syn::LitBool>;

#[allow(clippy::large_enum_variant)]
pub enum ErrorCountOption {
    Name(ErrorCountNameOption),
    Visibility(ErrorCountVisibilityOption),
    SkipCleared(ErrorCountSkipClearedOption),
}

impl ErrorCountOption {
    pub fn as_str(&self) -> &str {
        use syn::token::Token;
        match self {
            ErrorCountOption::Name(_) => <kw::name>::display(),
            ErrorCountOption::Visibility(_) => <kw::visibility>::display(),
            ErrorCountOption::SkipCleared(_) => <kw::skip_cleared>::display(),
        }
    }
}

impl Parse for ErrorCountOption {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        if ErrorCountNameOption::peek(input) {
            Ok(input.parse_as(ErrorCountOption::Name)?)
        } else if ErrorCountVisibilityOption::peek(input) {
            Ok(input.parse_as(ErrorCountOption::Visibility)?)
        } else if ErrorCountSkipClearedOption::peek(input) {
            Ok(input.parse_as(ErrorCountOption::SkipCleared)?)
        } else {
            let err = format!("invalid error_count option: {}", input);
            Err(input.error(err))
        }
    }
}
