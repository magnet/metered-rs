//! Parsed OpenMetrics text exposition.
//!
//! This module intentionally implements the subset of the OpenMetrics text
//! format that metered writes: metadata directives, samples, labels, exemplars,
//! and the terminal `# EOF`. It is a structural test/inspection API, not a
//! PromQL engine.

use metered::MetricType;
use std::fmt;

type Labels = Vec<(String, String)>;

/// A parsed OpenMetrics document.
///
/// `#[non_exhaustive]` (as are the other parse-model types): these are parser
/// *output*, so fields may be added without a breaking change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenMetricsDocument {
    /// Metric families found from HELP/TYPE/UNIT directives and samples.
    pub families: Vec<OpenMetricsFamily>,
    /// Sample lines in document order.
    pub samples: Vec<OpenMetricsSample>,
    /// Whether the document contained a terminal `# EOF` line.
    pub has_eof: bool,
}

/// Metadata for one OpenMetrics family.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenMetricsFamily {
    /// Family name before sample suffixes.
    pub name: String,
    /// Optional HELP text.
    pub help: Option<String>,
    /// Optional TYPE.
    pub metric_type: Option<MetricType>,
    /// Optional UNIT.
    pub unit: Option<String>,
}

/// One parsed sample line.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenMetricsSample {
    /// Sample name as written (`*_total`, `*_bucket`, etc.).
    pub name: String,
    /// Labels in document order.
    pub labels: Labels,
    /// Sample value token, kept as text to avoid changing precision.
    pub value: String,
    /// Optional exemplar.
    pub exemplar: Option<OpenMetricsExemplar>,
}

/// One parsed exemplar attached to a sample.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct OpenMetricsExemplar {
    /// Exemplar labels in document order.
    pub labels: Labels,
    /// Exemplar value token.
    pub value: String,
    /// Optional timestamp token.
    pub timestamp: Option<String>,
}

/// Error returned by [`OpenMetricsDocument::parse`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseOpenMetricsError {
    line: usize,
    message: String,
}

impl ParseOpenMetricsError {
    fn new(line: usize, message: impl Into<String>) -> Self {
        ParseOpenMetricsError {
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for ParseOpenMetricsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "OpenMetrics parse error on line {}: {}",
            self.line, self.message
        )
    }
}

impl std::error::Error for ParseOpenMetricsError {}

impl OpenMetricsDocument {
    /// Parses OpenMetrics text exposition.
    pub fn parse(input: &str) -> Result<Self, ParseOpenMetricsError> {
        let mut doc = OpenMetricsDocument::default();
        for (line_idx, raw_line) in input.lines().enumerate() {
            let line_no = line_idx + 1;
            let line = raw_line.trim_end();
            if line.is_empty() {
                continue;
            }
            if line == "# EOF" {
                doc.has_eof = true;
                continue;
            }
            if let Some(rest) = line.strip_prefix("# HELP ") {
                let (name, help) = split_once_ws(rest, line_no, "HELP")?;
                doc.family_mut(name).help = Some(unescape_help(help));
                continue;
            }
            if let Some(rest) = line.strip_prefix("# TYPE ") {
                let (name, metric_type) = split_once_ws(rest, line_no, "TYPE")?;
                doc.family_mut(name).metric_type = Some(parse_metric_type(metric_type, line_no)?);
                continue;
            }
            if let Some(rest) = line.strip_prefix("# UNIT ") {
                let (name, unit) = split_once_ws(rest, line_no, "UNIT")?;
                doc.family_mut(name).unit = Some(unit.to_owned());
                continue;
            }
            if line.starts_with('#') {
                continue;
            }

            let sample = parse_sample(line, line_no)?;
            let family_name = infer_family_name(&sample.name);
            doc.family_mut(&family_name);
            doc.samples.push(sample);
        }
        Ok(doc)
    }

    /// Finds a family by name.
    pub fn family(&self, name: &str) -> Option<&OpenMetricsFamily> {
        self.families.iter().find(|family| family.name == name)
    }

    /// Finds the first sample with `name`.
    pub fn sample(&self, name: &str) -> Option<&OpenMetricsSample> {
        self.samples.iter().find(|sample| sample.name == name)
    }

    /// Finds all samples with `name`.
    pub fn samples_named(&self, name: &str) -> Vec<&OpenMetricsSample> {
        self.samples
            .iter()
            .filter(|sample| sample.name == name)
            .collect()
    }

    fn family_mut(&mut self, name: &str) -> &mut OpenMetricsFamily {
        if let Some(index) = self.families.iter().position(|family| family.name == name) {
            return &mut self.families[index];
        }
        self.families.push(OpenMetricsFamily {
            name: name.to_owned(),
            ..Default::default()
        });
        self.families.last_mut().expect("family was just pushed")
    }
}

impl OpenMetricsSample {
    /// Returns the label value for `name`.
    pub fn label(&self, name: &str) -> Option<&str> {
        self.labels
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
    }
}

impl OpenMetricsExemplar {
    /// Returns the exemplar label value for `name`.
    pub fn label(&self, name: &str) -> Option<&str> {
        self.labels
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
    }
}

fn split_once_ws<'a>(
    rest: &'a str,
    line_no: usize,
    directive: &str,
) -> Result<(&'a str, &'a str), ParseOpenMetricsError> {
    rest.split_once(char::is_whitespace)
        .map(|(name, value)| (name, value.trim_start()))
        .filter(|(name, value)| !name.is_empty() && !value.is_empty())
        .ok_or_else(|| {
            ParseOpenMetricsError::new(line_no, format!("invalid {directive} directive"))
        })
}

fn parse_metric_type(value: &str, line_no: usize) -> Result<MetricType, ParseOpenMetricsError> {
    match value {
        "counter" => Ok(MetricType::Counter),
        "gauge" => Ok(MetricType::Gauge),
        "histogram" => Ok(MetricType::Histogram),
        "summary" => Ok(MetricType::Summary),
        "info" => Ok(MetricType::Info),
        "stateset" => Ok(MetricType::StateSet),
        "unknown" => Ok(MetricType::Unknown),
        "gaugehistogram" => Ok(MetricType::GaugeHistogram),
        other => Err(ParseOpenMetricsError::new(
            line_no,
            format!("unknown metric type `{other}`"),
        )),
    }
}

fn parse_sample(line: &str, line_no: usize) -> Result<OpenMetricsSample, ParseOpenMetricsError> {
    let (sample_part, exemplar_part) = split_exemplar(line);
    let (name_and_labels, value) = split_last_ws(sample_part, line_no, "sample")?;
    let (name, labels) = parse_name_and_labels(name_and_labels, line_no)?;
    let exemplar = exemplar_part
        .map(|part| parse_exemplar(part, line_no))
        .transpose()?;

    Ok(OpenMetricsSample {
        name: name.to_owned(),
        labels,
        value: value.to_owned(),
        exemplar,
    })
}

/// Splits a sample line from its trailing exemplar at the first ` # ` that
/// sits *outside* quoted label values (a label value may legally contain
/// `" # "`).
fn split_exemplar(line: &str) -> (&str, Option<&str>) {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut escaped = false;
    for (index, &byte) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_quotes => escaped = true,
            b'"' => in_quotes = !in_quotes,
            b' ' if !in_quotes && bytes[index..].starts_with(b" # ") => {
                return (&line[..index], Some(&line[index + 3..]));
            }
            _ => {}
        }
    }
    (line, None)
}

fn split_last_ws<'a>(
    input: &'a str,
    line_no: usize,
    what: &str,
) -> Result<(&'a str, &'a str), ParseOpenMetricsError> {
    input
        .rsplit_once(char::is_whitespace)
        .map(|(left, right)| (left.trim_end(), right.trim()))
        .filter(|(left, right)| !left.is_empty() && !right.is_empty())
        .ok_or_else(|| ParseOpenMetricsError::new(line_no, format!("invalid {what} line")))
}

fn parse_name_and_labels(
    input: &str,
    line_no: usize,
) -> Result<(&str, Labels), ParseOpenMetricsError> {
    match input.split_once('{') {
        Some((name, rest)) => {
            let label_block = rest
                .strip_suffix('}')
                .ok_or_else(|| ParseOpenMetricsError::new(line_no, "unterminated label block"))?;
            Ok((name, parse_labels(label_block, line_no)?))
        }
        None => Ok((input, Vec::new())),
    }
}

fn parse_exemplar(
    input: &str,
    line_no: usize,
) -> Result<OpenMetricsExemplar, ParseOpenMetricsError> {
    let (labels_part, value_and_ts) = input
        .strip_prefix('{')
        .and_then(|rest| rest.split_once("} "))
        .ok_or_else(|| ParseOpenMetricsError::new(line_no, "invalid exemplar"))?;
    let mut tokens = value_and_ts.split_whitespace();
    let value = tokens
        .next()
        .ok_or_else(|| ParseOpenMetricsError::new(line_no, "missing exemplar value"))?
        .to_owned();
    let timestamp = tokens.next().map(ToOwned::to_owned);
    if tokens.next().is_some() {
        return Err(ParseOpenMetricsError::new(
            line_no,
            "too many exemplar tokens",
        ));
    }
    Ok(OpenMetricsExemplar {
        labels: parse_labels(labels_part, line_no)?,
        value,
        timestamp,
    })
}

fn parse_labels(input: &str, line_no: usize) -> Result<Labels, ParseOpenMetricsError> {
    crate::lex::parse_label_pairs(input, crate::lex::Strictness::OpenMetrics)
        .map_err(|error| ParseOpenMetricsError::new(line_no, error.message))
}

fn unescape_help(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(ch) = chars.next() {
        match (ch, chars.clone().next()) {
            ('\\', Some('n')) => {
                let _ = chars.next();
                out.push('\n');
            }
            ('\\', Some('\\')) => {
                let _ = chars.next();
                out.push('\\');
            }
            _ => out.push(ch),
        }
    }
    out
}

fn infer_family_name(sample_name: &str) -> String {
    for suffix in ["_bucket", "_sum", "_count", "_total", "_info"] {
        if let Some(base) = sample_name.strip_suffix(suffix) {
            return base.to_owned();
        }
    }
    sample_name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_metric_type_accepts_unknown() {
        assert_eq!(parse_metric_type("unknown", 1), Ok(MetricType::Unknown));
    }

    #[test]
    fn parse_metric_type_accepts_gauge_histogram() {
        assert_eq!(
            parse_metric_type("gaugehistogram", 1),
            Ok(MetricType::GaugeHistogram)
        );
    }

    #[test]
    fn exemplar_split_ignores_hash_inside_quoted_label_values() {
        let doc =
            OpenMetricsDocument::parse("weird{path=\"a # b\"} 1 # {trace_id=\"abc\"} 0.5\n# EOF\n")
                .unwrap();
        let sample = doc.sample("weird").unwrap();
        assert_eq!(sample.label("path"), Some("a # b"));
        let exemplar = sample.exemplar.as_ref().unwrap();
        assert_eq!(exemplar.label("trace_id"), Some("abc"));
        assert_eq!(exemplar.value, "0.5");

        // A quoted ` # ` with no real exemplar stays part of the sample.
        let doc = OpenMetricsDocument::parse("weird{path=\"a # b\"} 1\n# EOF\n").unwrap();
        assert!(doc.sample("weird").unwrap().exemplar.is_none());
    }

    #[test]
    fn parses_an_empty_exemplar_labelset() {
        let doc =
            OpenMetricsDocument::parse("latency_bucket{le=\"+Inf\"} 1 # {} 0.5\n# EOF\n").unwrap();
        let exemplar = doc.sample("latency_bucket").unwrap().exemplar.as_ref();
        let exemplar = exemplar.expect("exemplar parsed");
        assert!(exemplar.labels.is_empty());
        assert_eq!(exemplar.value, "0.5");
    }
}
