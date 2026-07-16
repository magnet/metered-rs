//! Lenient Prometheus-text parsing and the [`TextSourceTree`] re-encoder.
//!
//! The classic Prometheus text format (as emitted by e.g. `serde_prometheus`)
//! is looser than OpenMetrics: metadata lines are optional, label braces may
//! contain spaces around `=`, and counters do not carry the `_total` suffix.
//! This module parses that dialect into raw samples and re-emits them through
//! a [`MetricTree`] as a **normalizing re-encode**: names and labels are
//! preserved exactly, while values re-render from their parsed numeric form
//! and sample timestamps are dropped (see [`TextSourceTree`] for the precise
//! contract). This is the foundation for serving metrics from a foreign
//! (older or non-metered) source on a metered scrape endpoint.

use metered::{MetricSampleValue, MetricSchema, MetricTree, MetricValues, compose_labels};
use std::fmt;

/// One sample parsed from classic Prometheus text, kept as raw text tokens so
/// re-emission is lossless.
///
/// `#[non_exhaustive]`: parser output, so fields may be added without a
/// breaking change.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RawSample {
    /// Sample name exactly as it appeared (no `_total` normalization).
    pub name: String,
    /// Labels in source order.
    pub labels: Vec<(String, String)>,
    /// The value token, untouched (`12`, `0.5`, `NaN`, `+Inf`, ...).
    pub value: String,
    /// Optional trailing timestamp token.
    pub timestamp: Option<String>,
}

/// A parse failure, with the 1-based source line.
#[derive(Debug)]
pub struct ParsePrometheusTextError {
    line: usize,
    message: String,
}

impl fmt::Display for ParsePrometheusTextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParsePrometheusTextError {}

fn err(line: usize, message: impl Into<String>) -> ParsePrometheusTextError {
    ParsePrometheusTextError {
        line,
        message: message.into(),
    }
}

/// Parses classic Prometheus text exposition into raw samples.
///
/// Lenient by design: `# ...` comment/metadata lines and blank lines are
/// skipped; spaces are tolerated around `=` and after `,` inside label braces;
/// values are kept as raw tokens (including `NaN`, `+Inf`, `-Inf`).
pub fn parse_prometheus_text(text: &str) -> Result<Vec<RawSample>, ParsePrometheusTextError> {
    let mut samples = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        samples.push(parse_sample_line(line, line_no)?);
    }
    Ok(samples)
}

fn parse_sample_line(line: &str, line_no: usize) -> Result<RawSample, ParsePrometheusTextError> {
    // name[{labels}] value [timestamp]
    let (name_and_labels, rest) = match line.find('{') {
        Some(brace) => {
            let close = crate::lex::find_closing_brace(line, brace)
                .ok_or_else(|| err(line_no, "unterminated label braces"))?;
            (line[..close + 1].trim(), line[close + 1..].trim())
        }
        None => {
            let mut parts = line.splitn(2, char::is_whitespace);
            let name = parts.next().unwrap_or_default();
            (name, parts.next().unwrap_or("").trim())
        }
    };

    let (name, labels) = match name_and_labels.find('{') {
        Some(brace) => {
            let name = name_and_labels[..brace].trim();
            let body = &name_and_labels[brace + 1..name_and_labels.len() - 1];
            (name, parse_labels(body, line_no)?)
        }
        None => (name_and_labels, Vec::new()),
    };
    if name.is_empty() {
        return Err(err(line_no, "missing sample name"));
    }

    let mut tokens = rest.split_whitespace();
    let value = tokens
        .next()
        .ok_or_else(|| err(line_no, "missing sample value"))?
        .to_owned();
    let timestamp = tokens.next().map(ToOwned::to_owned);
    if tokens.next().is_some() {
        return Err(err(line_no, "unexpected trailing tokens"));
    }

    Ok(RawSample {
        name: name.to_owned(),
        labels,
        value,
        timestamp,
    })
}

fn parse_labels(
    body: &str,
    line_no: usize,
) -> Result<Vec<(String, String)>, ParsePrometheusTextError> {
    crate::lex::parse_label_pairs(body, crate::lex::Strictness::Lenient)
        .map_err(|error| err(line_no, error.message))
}

/// A [`MetricTree`] that re-emits samples parsed from a foreign
/// Prometheus-text producer as a **normalizing re-encode**: series identity is
/// preserved exactly, but the output is re-rendered from the parsed form, not
/// copied byte-for-byte.
///
/// Built for migrations: an application moving to metered can keep serving the
/// metrics of an older metrics stack (or any sidecar producing Prometheus
/// text) on the same scrape endpoint, by mounting one `TextSourceTree` next to
/// its native trees. Samples stay untyped -- no `# TYPE` lines are invented,
/// no `_total` normalization -- so existing dashboards keep working
/// unmodified.
///
/// # What is preserved, what is normalized
///
/// Preserved exactly:
/// - **Names**: emitted as parsed, no suffix or prefix normalization.
/// - **Labels**: names, values, and their source order.
/// - The **integral/float distinction**: `12` re-renders as `12`, not `12.0`.
///
/// Normalized or dropped by the re-encode:
/// - **Value tokens** re-render from their parsed numeric form: scientific
///   notation expands (`1.5e3` becomes `1500`), an integral negative zero
///   collapses to `0`, and equivalent spellings of the same number converge
///   on one canonical rendering.
/// - **Sample timestamps** are dropped: the collected model carries no
///   per-sample timestamp, so a trailing timestamp token never reaches the
///   output. The scraper assigns its own scrape time, which is what both
///   Prometheus and VictoriaMetrics do with timestamp-less samples.
/// - **Unparseable value tokens** (and, when the whole document fails to
///   parse, unparseable lines) are dropped rather than failing the scrape.
/// - **Label collisions with inherited labels**: a foreign label sharing a
///   name with a label inherited from the mount point (e.g. a registry
///   constant label) replaces the inherited pair in place, per
///   [`metered::compose_labels`] -- one pair per name on the wire.
///
/// `describe` is intentionally empty (foreign samples are untyped);
/// [`housekeep`](MetricTree::housekeep) drives an optional hook so the foreign
/// source can run its own per-scrape maintenance (e.g. swapping interval
/// histograms) exactly once per scrape cycle.
pub struct TextSourceTree {
    source: Box<dyn Fn() -> String + Send + Sync>,
    housekeep: Option<Box<dyn Fn() + Send + Sync>>,
}

impl TextSourceTree {
    /// Wraps a Prometheus-text producer: any classic Prometheus/OpenMetrics
    /// text source -- a foreign exporter, a sidecar, or a metered 0.9 registry's
    /// `serde_prometheus` output. Series identity survives the re-encode, so
    /// e.g. a metered 0.9 HDR summary (`name{quantile="…"}`, `name_sum`,
    /// `name_count`) re-appears as the same summary-shaped series during
    /// migration -- subject to the normalizations documented on
    /// [`TextSourceTree`] (values re-render from parsed form, sample
    /// timestamps are dropped).
    ///
    /// ```
    /// use metered_om::{OpenMetricsExt, TextSourceTree};
    ///
    /// // e.g. a metered 0.9 registry serialized via `serde_prometheus`.
    /// let nine = || "response_time{quantile=\"0.99\"} 250.5\n\
    ///                response_time_sum 1000\n\
    ///                response_time_count 7\n"
    ///     .to_owned();
    /// let passthrough = TextSourceTree::new(nine);
    /// let text = passthrough.encode_to_string().unwrap();
    /// assert!(text.contains("response_time{quantile=\"0.99\"} 250.5"));
    /// assert!(text.contains("response_time_count 7"));
    /// ```
    pub fn new(source: impl Fn() -> String + Send + Sync + 'static) -> Self {
        TextSourceTree {
            source: Box::new(source),
            housekeep: None,
        }
    }

    /// Adds a per-scrape maintenance hook, run by
    /// [`housekeep`](MetricTree::housekeep) (once per scrape cycle when driven
    /// by a registry or snapshot cache).
    pub fn with_housekeep(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.housekeep = Some(Box::new(hook));
        self
    }
}

impl fmt::Debug for TextSourceTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextSourceTree")
            .field("housekeep", &self.housekeep.is_some())
            .finish()
    }
}

impl MetricTree for TextSourceTree {
    fn describe(&self, _name: &str, _labels: &[(&str, &str)], _schema: &mut MetricSchema) {
        // Foreign samples are deliberately undeclared: classic Prometheus text
        // has no TYPE metadata and inventing one would change the exposition.
    }

    fn collect(&self, _name: &str, inherited: &[(&str, &str)], values: &mut MetricValues) {
        let text = (self.source)();
        let samples = match parse_prometheus_text(&text) {
            Ok(samples) => samples,
            // A scrape must not fail because the foreign source glitched;
            // salvage line-by-line.
            Err(_) => text
                .lines()
                .enumerate()
                .filter(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .filter_map(|(i, l)| parse_sample_line(l.trim(), i + 1).ok())
                .collect(),
        };
        for sample in samples {
            let Some(value) = sample_value(&sample.value) else {
                continue;
            };
            // Inner-wins composition: a foreign label colliding with an
            // inherited (e.g. registry constant) label replaces it in place,
            // so a series never carries duplicate label names on the wire.
            let labels = compose_labels(
                inherited,
                sample.labels.iter().map(|(k, v)| (k.as_str(), v.as_str())),
            );
            values.sample(&sample.name, &labels, value);
        }
    }

    fn housekeep(&self) {
        if let Some(hook) = &self.housekeep {
            hook();
        }
    }

    fn needs_housekeep(&self) -> bool {
        self.housekeep.is_some()
    }
}

/// Converts a raw value token, preserving the integral/float distinction so
/// re-rendered text matches the source (`12` stays `12`, not `12.0`).
fn sample_value(token: &str) -> Option<MetricSampleValue> {
    if let Ok(unsigned) = token.parse::<u64>() {
        return Some(MetricSampleValue::from(unsigned));
    }
    if let Ok(signed) = token.parse::<i64>() {
        return Some(MetricSampleValue::from(signed));
    }
    match token {
        "+Inf" => return Some(MetricSampleValue::from(f64::INFINITY)),
        "-Inf" => return Some(MetricSampleValue::from(f64::NEG_INFINITY)),
        _ => {}
    }
    token.parse::<f64>().ok().map(MetricSampleValue::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_samples_and_labels_with_spaces() {
        let text = "\
legacy_hit_count{method = \"test\"} 12
legacy_response_time{method = \"test\", quantile = \"0.95\"} 250
up 1
";
        let samples = parse_prometheus_text(text).unwrap();
        assert_eq!(samples.len(), 3);
        assert_eq!(samples[0].name, "legacy_hit_count");
        assert_eq!(
            samples[0].labels,
            vec![("method".to_owned(), "test".to_owned())]
        );
        assert_eq!(samples[0].value, "12");
        assert_eq!(
            samples[1].labels,
            vec![
                ("method".to_owned(), "test".to_owned()),
                ("quantile".to_owned(), "0.95".to_owned())
            ]
        );
        assert_eq!(samples[2].name, "up");
        assert!(samples[2].labels.is_empty());
    }

    #[test]
    fn skips_comments_blank_lines_and_metadata() {
        let text = "\
# HELP something Something.
# TYPE something counter

something 3 1700000000
";
        let samples = parse_prometheus_text(text).unwrap();
        assert_eq!(samples.len(), 1);
        // A trailing timestamp is parsed and preserved.
        assert_eq!(samples[0].timestamp.as_deref(), Some("1700000000"));
    }

    #[test]
    fn preserves_escapes_and_special_values() {
        let text = "\
errs{kind=\"bad \\\"quote\\\"\", path=\"a\\\\b\"} 1
latency_max NaN
saturation +Inf
";
        let samples = parse_prometheus_text(text).unwrap();
        assert_eq!(samples[0].labels[0].1, "bad \"quote\"");
        assert_eq!(samples[0].labels[1].1, "a\\b");
        assert_eq!(samples[1].value, "NaN");
        assert_eq!(samples[2].value, "+Inf");
    }

    #[test]
    fn rejects_garbage_lines_with_line_numbers() {
        let err = parse_prometheus_text("this is not a metric line at all {").unwrap_err();
        assert!(err.to_string().contains("line 1"), "{err}");
    }

    use metered::{MetricTree, MetricValues};

    fn legacy_text() -> &'static str {
        "legacy_hit_count{method = \"test\", service = \"orders\"} 12\n\
         legacy_response_time{quantile = \"0.95\", service = \"orders\"} 250.5\n"
    }

    #[test]
    fn text_source_tree_reemits_samples_untouched() {
        let tree = TextSourceTree::new(|| legacy_text().to_owned());

        let mut values = MetricValues::new();
        tree.collect("", &[], &mut values);

        let samples = values.samples();
        assert_eq!(samples.len(), 2);
        // Name untouched: no `_total`, no prefix.
        assert_eq!(samples[0].name, "legacy_hit_count");
        // Integral values stay integral (no `12.0` drift in the output).
        assert_eq!(samples[0].value.to_string(), "12");
        assert_eq!(samples[1].value.to_string(), "250.5");
        // Labels preserved (order included).
        assert_eq!(
            samples[0].labels[0],
            ("method".to_owned(), "test".to_owned())
        );
    }

    #[test]
    fn text_source_tree_runs_housekeep_hook_once_per_housekeep() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let swaps = Arc::new(AtomicUsize::new(0));
        let swaps_in_hook = swaps.clone();
        let tree = TextSourceTree::new(String::new).with_housekeep(move || {
            swaps_in_hook.fetch_add(1, Ordering::Relaxed);
        });

        assert!(tree.needs_housekeep());
        tree.housekeep();
        tree.housekeep();
        assert_eq!(swaps.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn text_source_tree_swallows_unparseable_lines_but_keeps_good_ones() {
        let tree = TextSourceTree::new(|| "ok 1\ntotal garbage {{{{\n".to_owned());
        let mut values = MetricValues::new();
        tree.collect("", &[], &mut values);
        // Good line survives; the bad one is dropped (scrape must not 500).
        assert_eq!(values.samples().len(), 1);
    }

    #[test]
    fn passes_through_a_legacy_0_9_summary() {
        let nine = || {
            "response_time{quantile=\"0.99\",service=\"orders\"} 250.5\n\
                       response_time_sum{service=\"orders\"} 1000\n\
                       response_time_count{service=\"orders\"} 7\n"
                .to_owned()
        };
        let tree = super::TextSourceTree::new(nine);
        let mut values = MetricValues::new();
        tree.collect("", &[], &mut values);
        assert!(values.samples().iter().any(|s| s.name == "response_time"
            && s.labels.iter().any(|(k, v)| k == "quantile" && v == "0.99")));
        assert!(
            values
                .samples()
                .iter()
                .any(|s| s.name == "response_time_count")
        );
    }

    #[test]
    fn rendered_output_matches_source_lines() {
        use crate::OpenMetricsExt;

        let tree = TextSourceTree::new(|| legacy_text().to_owned());
        let rendered = tree.encode_to_string().unwrap();
        assert!(rendered.contains("legacy_hit_count{method=\"test\",service=\"orders\"} 12"));
        assert!(
            rendered.contains("legacy_response_time{quantile=\"0.95\",service=\"orders\"} 250.5")
        );
        // No TYPE lines were invented for foreign samples.
        assert!(!rendered.contains("# TYPE legacy_hit_count"));
    }

    /// Wire regression for inherited-label collisions: a foreign sample whose
    /// label shares a name with the registry's constant label must render
    /// exactly one pair for that name (the foreign value), never duplicate
    /// label names in one series.
    #[test]
    fn foreign_label_colliding_with_registry_constant_renders_one_pair() {
        use crate::OpenMetricsRegistryExt;
        use metered::Registry;

        let tree =
            TextSourceTree::new(|| "legacy_hits{service=\"legacy\",method=\"get\"} 4\n".to_owned());
        let mut registry = Registry::new();
        registry.label("service", "orders");
        registry.register(metered::entry::metric("legacy").source(&tree));

        let rendered = registry.encode_to_string().unwrap();
        assert!(
            rendered.contains("legacy_hits{service=\"legacy\",method=\"get\"} 4"),
            "the foreign pair wins the collision, in the inherited position:\n{rendered}"
        );
        assert_eq!(
            rendered.matches("service=").count(),
            1,
            "exactly one `service` pair on the wire:\n{rendered}"
        );
    }

    /// Pins the documented normalizing-re-encode contract: sample timestamps
    /// are dropped and value tokens re-render from their parsed form. If this
    /// test starts failing, the type's documented contract changed -- update
    /// the [`TextSourceTree`] docs in the same commit.
    #[test]
    fn reencode_drops_timestamps_and_normalizes_value_tokens() {
        use crate::OpenMetricsExt;

        let tree = TextSourceTree::new(|| {
            "stamped_total 3 1700000000\n\
             sci_notation 1.5e3\n"
                .to_owned()
        });
        let rendered = tree.encode_to_string().unwrap();

        // The trailing timestamp token never reaches the output.
        assert!(
            rendered.contains("stamped_total 3\n"),
            "timestamp must be dropped:\n{rendered}"
        );
        assert!(!rendered.contains("1700000000"), "{rendered}");

        // A scientific-notation token re-renders from its parsed value.
        assert!(
            rendered.contains("sci_notation 1500\n"),
            "sci-notation must normalize:\n{rendered}"
        );
        assert!(!rendered.contains("1.5e3"), "{rendered}");
    }
}
