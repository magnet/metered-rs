//! Span-field capture: the typed values recorded on a span, available to span
//! metrics and exemplar providers at close time.
//!
//! Fields are captured **typed** from the span's `Attributes` / `record` calls
//! -- an `i64` stays an `i64`, a `&str` is stored once as text -- so a typed
//! label struct reads them back without a `to_string` -> `FromStr` round-trip,
//! and a value that cannot convert is a visible error rather than a silently
//! defaulted label.

use std::collections::HashMap;
use std::fmt;
use tracing::field::{Field, Visit};

/// One captured span-field value, preserving the type it was recorded with.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum FieldValue {
    /// A string value (`record_str`).
    Str(String),
    /// A signed integer (`record_i64`).
    I64(i64),
    /// An unsigned integer (`record_u64`).
    U64(u64),
    /// A boolean (`record_bool`).
    Bool(bool),
    /// A float (`record_f64`).
    F64(f64),
    /// The `Debug`/`Display` rendering of a non-primitive value
    /// (`record_debug`; also how `tracing::field::display` values arrive).
    Debug(String),
}

impl FieldValue {
    /// Renders the value as label text (what a stringly label consumer sees).
    pub fn to_text(&self) -> String {
        match self {
            FieldValue::Str(text) | FieldValue::Debug(text) => text.clone(),
            FieldValue::I64(value) => value.to_string(),
            FieldValue::U64(value) => value.to_string(),
            FieldValue::Bool(value) => value.to_string(),
            FieldValue::F64(value) => value.to_string(),
        }
    }

    /// The captured text, when the value was recorded as text.
    pub(crate) fn as_text(&self) -> Option<&str> {
        match self {
            FieldValue::Str(text) | FieldValue::Debug(text) => Some(text),
            _ => None,
        }
    }
}

/// Why a captured [`FieldValue`] could not convert into a label type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldValueError {
    /// The label type the conversion targeted (e.g. `u64`).
    pub expected: &'static str,
}

impl FieldValueError {
    fn new(expected: &'static str) -> Self {
        FieldValueError { expected }
    }
}

impl fmt::Display for FieldValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "value does not convert to `{}`", self.expected)
    }
}

impl std::error::Error for FieldValueError {}

/// Converts a captured [`FieldValue`] into a typed label value.
///
/// [`from_text`](FromFieldValue::from_text) is the one required method: a
/// custom label type parses its text form and is done. The provided
/// [`from_field_value`](FromFieldValue::from_field_value) renders the captured
/// value as text and parses it; the standard label types (strings, integers,
/// `bool`, floats) override it with **typed** fast paths -- a natively
/// recorded `u64` becomes a `u64` label with no string round-trip.
///
/// ```
/// # use metered_tracing::{FieldValueError, FromFieldValue};
/// # #[derive(Default)]
/// # struct Status;
/// # impl std::str::FromStr for Status {
/// #     type Err = ();
/// #     fn from_str(_: &str) -> Result<Self, ()> { Ok(Status) }
/// # }
/// impl FromFieldValue for Status {
///     fn from_text(text: &str) -> Result<Self, FieldValueError> {
///         text.parse().map_err(|()| FieldValueError { expected: "Status" })
///     }
/// }
/// ```
pub trait FromFieldValue: Sized {
    /// Converts label text (e.g. a configured `default = "..."` literal) into
    /// this label type.
    fn from_text(text: &str) -> Result<Self, FieldValueError>;

    /// Converts a captured value into this label type. Defaults to parsing the
    /// value's text rendering; the primitive impls override it with typed fast
    /// paths.
    fn from_field_value(value: &FieldValue) -> Result<Self, FieldValueError> {
        Self::from_text(&value.to_text())
    }
}

impl FromFieldValue for String {
    fn from_text(text: &str) -> Result<Self, FieldValueError> {
        Ok(text.to_owned())
    }

    /// A string label faithfully holds any captured value.
    fn from_field_value(value: &FieldValue) -> Result<Self, FieldValueError> {
        Ok(value.to_text())
    }
}

impl FromFieldValue for bool {
    fn from_text(text: &str) -> Result<Self, FieldValueError> {
        text.parse().map_err(|_| FieldValueError::new("bool"))
    }

    fn from_field_value(value: &FieldValue) -> Result<Self, FieldValueError> {
        match value {
            FieldValue::Bool(value) => Ok(*value),
            other => other
                .as_text()
                .and_then(|text| text.parse().ok())
                .ok_or_else(|| FieldValueError::new("bool")),
        }
    }
}

macro_rules! from_field_value_int {
    ($($ty:ty),*) => {
        $(
            impl FromFieldValue for $ty {
                fn from_text(text: &str) -> Result<Self, FieldValueError> {
                    text.parse()
                        .map_err(|_| FieldValueError::new(stringify!($ty)))
                }

                fn from_field_value(value: &FieldValue) -> Result<Self, FieldValueError> {
                    match value {
                        FieldValue::I64(value) => (*value)
                            .try_into()
                            .map_err(|_| FieldValueError::new(stringify!($ty))),
                        FieldValue::U64(value) => (*value)
                            .try_into()
                            .map_err(|_| FieldValueError::new(stringify!($ty))),
                        other => other
                            .as_text()
                            .and_then(|text| text.parse().ok())
                            .ok_or_else(|| FieldValueError::new(stringify!($ty))),
                    }
                }
            }
        )*
    };
}

from_field_value_int!(i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

macro_rules! from_field_value_float {
    ($($ty:ty),*) => {
        $(
            impl FromFieldValue for $ty {
                fn from_text(text: &str) -> Result<Self, FieldValueError> {
                    text.parse()
                        .map_err(|_| FieldValueError::new(stringify!($ty)))
                }

                fn from_field_value(value: &FieldValue) -> Result<Self, FieldValueError> {
                    match value {
                        FieldValue::F64(value) => Ok(*value as $ty),
                        FieldValue::I64(value) => Ok(*value as $ty),
                        FieldValue::U64(value) => Ok(*value as $ty),
                        other => other
                            .as_text()
                            .and_then(|text| text.parse().ok())
                            .ok_or_else(|| FieldValueError::new(stringify!($ty))),
                    }
                }
            }
        )*
    };
}

from_field_value_float!(f32, f64);

/// Captured span fields available to span metrics and exemplar providers.
#[derive(Clone, Debug, Default)]
pub struct SpanFields {
    values: HashMap<String, FieldValue>,
}

impl SpanFields {
    /// Returns a captured field's typed value by name.
    pub fn value(&self, name: &str) -> Option<&FieldValue> {
        self.values.get(name)
    }

    /// Returns a captured field rendered as label text.
    pub fn text(&self, name: &str) -> Option<String> {
        self.value(name).map(FieldValue::to_text)
    }

    fn record(&mut self, field: &Field, value: FieldValue) {
        self.values.insert(field.name().to_owned(), value);
    }

    #[cfg(test)]
    pub(crate) fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, FieldValue)>) -> Self {
        SpanFields {
            values: pairs
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        }
    }
}

/// The `tracing` visitor that fills a [`SpanFields`] from span attributes and
/// later `record` calls, preserving the recorded type.
pub(crate) struct SpanFieldVisitor<'a> {
    fields: &'a mut SpanFields,
}

impl<'a> SpanFieldVisitor<'a> {
    pub(crate) fn new(fields: &'a mut SpanFields) -> Self {
        SpanFieldVisitor { fields }
    }
}

impl Visit for SpanFieldVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.fields
            .record(field, FieldValue::Debug(format!("{value:?}")));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.record(field, FieldValue::I64(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.record(field, FieldValue::U64(value));
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields.record(field, FieldValue::Bool(value));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.fields.record(field, FieldValue::F64(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.record(field, FieldValue::Str(value.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_convert_without_a_string_round_trip() {
        assert_eq!(u64::from_field_value(&FieldValue::U64(4242)), Ok(4242));
        assert_eq!(i64::from_field_value(&FieldValue::I64(-1)), Ok(-1));
        assert_eq!(bool::from_field_value(&FieldValue::Bool(true)), Ok(true));
        assert_eq!(
            String::from_field_value(&FieldValue::Str("GET".to_owned())),
            Ok("GET".to_owned())
        );
        // Text forms (e.g. from a Display-based opener) still parse.
        assert_eq!(
            u64::from_field_value(&FieldValue::Debug("7".to_owned())),
            Ok(7)
        );
    }

    #[test]
    fn out_of_contract_values_error_instead_of_defaulting() {
        assert!(u64::from_field_value(&FieldValue::Str("not-a-number".to_owned())).is_err());
        assert!(u64::from_field_value(&FieldValue::I64(-4)).is_err());
        assert!(bool::from_field_value(&FieldValue::U64(2)).is_err());
    }

    #[test]
    fn custom_type_implementing_only_from_text_converts_through_both_entry_points() {
        #[derive(Debug, PartialEq)]
        struct Status(String);

        impl FromFieldValue for Status {
            fn from_text(text: &str) -> Result<Self, FieldValueError> {
                if text.is_empty() {
                    return Err(FieldValueError::new("Status"));
                }
                Ok(Status(text.to_owned()))
            }
        }

        assert_eq!(Status::from_text("OK"), Ok(Status("OK".to_owned())));
        // The default `from_field_value` routes through `from_text` -- no
        // recursion back into the default `from_field_value`.
        assert_eq!(
            Status::from_field_value(&FieldValue::Str("OK".to_owned())),
            Ok(Status("OK".to_owned()))
        );
        assert_eq!(
            Status::from_field_value(&FieldValue::U64(7)),
            Ok(Status("7".to_owned()))
        );
        assert!(Status::from_field_value(&FieldValue::Str(String::new())).is_err());
    }
}
