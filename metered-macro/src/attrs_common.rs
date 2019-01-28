#![macro_use]


use std::convert::AsRef;
use syn::parse::{Parse, ParseStream};
use syn::Result;


/// A Key Value option.
pub struct KVOption<K: Parse + AsRef<str>, V: Parse> {
    pub key: K,
    pub colon_token: Option<syn::Token![:]>,
    pub eq_token: syn::Token![=],
    pub value: V,
}

impl<K: Parse + AsRef<str>, V: Parse> Parse for KVOption<K, V> {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(KVOption {
            key: input.parse()?,
            colon_token: input.parse()?,
            eq_token: input.parse()?,
            value: input.parse()?,
        })
    }
}


/// Single or [MultipleA, MultipleB] values.
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





macro_rules! custom_keyword {
    ($name:ident, $keyword:tt) => {       
        pub struct $name {
            pub ident: syn::Ident,
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                stringify!($keyword)
            }
        }

        impl Parse for $name {
            fn parse(input: ParseStream) -> Result<Self> {
                let fork = input.fork();
                let ident = fork.parse::<syn::Ident>()?;
                if ident == stringify!($keyword) {
                    let _ = input.parse::<syn::Ident>();
                    Ok($name { ident })
                } else {
                    Err(input.error(concat!("Not ", stringify!($keyword))))
                }
            }
        }
    };
}

macro_rules! token_keyword {
    ($name:ident, $keyword:tt) => {       
        pub struct $name {
            pub token: syn::Token![$keyword],
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                stringify!($keyword)
            }
        }

        impl Parse for $name {
            fn parse(input: ParseStream) -> Result<Self> {
                 Ok($name {
                    token: input.parse()?,
                 })
            }
        }
    };
}


/// An invocation handle that may be `bar::foo` or `bar::foo!`
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



pub trait ParseStreamExt {
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
