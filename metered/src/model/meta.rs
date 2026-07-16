//! Typed metric metadata used by registries, views, schema, and sinks.

use crate::values::MetricSampleValue;
use std::borrow::Cow;

/// One OpenMetrics metric-family name segment or registry prefix.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(Cow<'static, str>);

impl Name {
    /// Creates a name from any supported input.
    pub fn new(value: impl Into<Name>) -> Self {
        value.into()
    }

    /// Creates a `const`-friendly name from a static string.
    pub const fn literal(value: &'static str) -> Self {
        Name(Cow::Borrowed(value))
    }

    /// Returns the raw OpenMetrics name token.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Converts into an owned string.
    pub fn into_string(self) -> String {
        self.0.into_owned()
    }
}

impl From<&'static str> for Name {
    fn from(value: &'static str) -> Self {
        Name(Cow::Borrowed(value))
    }
}

impl From<String> for Name {
    fn from(value: String) -> Self {
        Name(Cow::Owned(value))
    }
}

impl From<Cow<'static, str>> for Name {
    fn from(value: Cow<'static, str>) -> Self {
        Name(value)
    }
}

/// OpenMetrics HELP text.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Help(Cow<'static, str>);

impl Help {
    /// Creates HELP text from any supported input.
    pub fn new(value: impl Into<Help>) -> Self {
        value.into()
    }

    /// Creates `const`-friendly HELP text from a static string.
    pub const fn literal(value: &'static str) -> Self {
        Help(Cow::Borrowed(value))
    }

    /// Returns the raw HELP text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for Help {
    fn from(value: &'static str) -> Self {
        Help(Cow::Borrowed(value))
    }
}

impl From<String> for Help {
    fn from(value: String) -> Self {
        Help(Cow::Owned(value))
    }
}

impl From<Cow<'static, str>> for Help {
    fn from(value: Cow<'static, str>) -> Self {
        Help(value)
    }
}

/// One metric label name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LabelName(Cow<'static, str>);

impl LabelName {
    /// Creates a label name from any supported input.
    pub fn new(value: impl Into<LabelName>) -> Self {
        value.into()
    }

    /// Creates a `const`-friendly label name from a static string.
    pub const fn literal(value: &'static str) -> Self {
        LabelName(Cow::Borrowed(value))
    }

    /// Returns the raw label name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Converts into an owned string.
    pub fn into_string(self) -> String {
        self.0.into_owned()
    }
}

impl From<&'static str> for LabelName {
    fn from(value: &'static str) -> Self {
        LabelName(Cow::Borrowed(value))
    }
}

impl From<String> for LabelName {
    fn from(value: String) -> Self {
        LabelName(Cow::Owned(value))
    }
}

impl From<Cow<'static, str>> for LabelName {
    fn from(value: Cow<'static, str>) -> Self {
        LabelName(value)
    }
}

/// OpenMetrics metric unit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Unit {
    /// Seconds.
    Seconds,
    /// Bytes.
    Bytes,
    /// Dimensionless item count.
    Items,
    /// Request count.
    Requests,
    /// Custom unit. Prefer built-ins when possible.
    Custom(Cow<'static, str>),
}

impl Unit {
    /// Returns the OpenMetrics `# UNIT` token.
    pub fn as_str(&self) -> &str {
        match self {
            Unit::Seconds => "seconds",
            Unit::Bytes => "bytes",
            Unit::Items => "items",
            Unit::Requests => "requests",
            Unit::Custom(value) => value.as_ref(),
        }
    }
}

impl From<&'static str> for Unit {
    fn from(value: &'static str) -> Self {
        match value {
            "seconds" => Unit::Seconds,
            "bytes" => Unit::Bytes,
            "items" => Unit::Items,
            "requests" => Unit::Requests,
            other => Unit::Custom(Cow::Borrowed(other)),
        }
    }
}

impl From<String> for Unit {
    fn from(value: String) -> Self {
        match value.as_str() {
            "seconds" => Unit::Seconds,
            "bytes" => Unit::Bytes,
            "items" => Unit::Items,
            "requests" => Unit::Requests,
            _ => Unit::Custom(Cow::Owned(value)),
        }
    }
}

/// Numeric scalar sample value used by counters, gauges, and external readers.
///
/// Scalars keep their integer/float shape so large integer counters and gauges
/// do not lose precision before reaching the sampled-value model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scalar {
    /// A signed integer scalar.
    Int(i64),
    /// An unsigned integer scalar.
    UInt(u64),
    /// A floating-point scalar.
    Float(f64),
}

impl Scalar {
    /// Creates a scalar from any supported numeric input.
    pub fn new(value: impl Into<Scalar>) -> Self {
        value.into()
    }

    /// Returns the scalar as an `f64`.
    pub fn as_f64(self) -> f64 {
        match self {
            Scalar::Int(value) => value as f64,
            Scalar::UInt(value) => value as f64,
            Scalar::Float(value) => value,
        }
    }
}

impl From<u64> for Scalar {
    fn from(value: u64) -> Self {
        Scalar::UInt(value)
    }
}

impl From<usize> for Scalar {
    fn from(value: usize) -> Self {
        Scalar::UInt(value as u64)
    }
}

impl From<i64> for Scalar {
    fn from(value: i64) -> Self {
        Scalar::Int(value)
    }
}

impl From<f64> for Scalar {
    fn from(value: f64) -> Self {
        Scalar::Float(value)
    }
}

impl From<Scalar> for MetricSampleValue {
    fn from(value: Scalar) -> Self {
        match value {
            Scalar::Int(value) => MetricSampleValue::from(value),
            Scalar::UInt(value) => MetricSampleValue::from(value),
            Scalar::Float(value) => MetricSampleValue::from(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_accepts_static_and_owned_strings() {
        let from_static = Name::from("requests");
        let from_owned = Name::from(String::from("queue_depth"));

        assert_eq!(from_static.as_str(), "requests");
        assert_eq!(from_owned.as_str(), "queue_depth");
    }

    #[test]
    fn help_is_separate_from_name_at_type_level() {
        let help = Help::from("Total requests");
        assert_eq!(help.as_str(), "Total requests");
    }

    #[test]
    fn unit_renders_openmetrics_tokens() {
        assert_eq!(Unit::Seconds.as_str(), "seconds");
        assert_eq!(Unit::Bytes.as_str(), "bytes");
        assert_eq!(Unit::Items.as_str(), "items");
        assert_eq!(Unit::Requests.as_str(), "requests");
        assert_eq!(Unit::Custom("widgets".into()).as_str(), "widgets");
    }

    #[test]
    fn scalar_accepts_integer_and_float_values() {
        assert_eq!(Scalar::from(42u64).as_f64(), 42.0);
        assert_eq!(Scalar::from(-7i64).as_f64(), -7.0);
        assert_eq!(Scalar::from(0.25f64).as_f64(), 0.25);
    }

    #[test]
    fn scalar_preserves_integer_sample_value_shape() {
        assert_eq!(
            MetricSampleValue::from(Scalar::from(u64::MAX)),
            MetricSampleValue::UInt(u64::MAX)
        );
        assert_eq!(
            MetricSampleValue::from(Scalar::from(i64::MIN)),
            MetricSampleValue::Int(i64::MIN)
        );
    }
}
