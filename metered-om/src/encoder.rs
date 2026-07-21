//! The OpenMetrics text encoder and the low-level line writers.

use metered::bucket_histogram::HistogramSnapshot;
use metered::exponential_histogram::ExponentialSnapshot;
use metered::{
    HistogramData, HistogramValue, MetricExemplar, MetricFamilySchema, MetricSample,
    MetricSampleValue, MetricSchema, MetricSink, MetricValues,
};
use std::collections::HashSet;
use std::fmt::{self, Display, Write};

/// How a histogram's buckets are rendered in the OpenMetrics text.
///
/// `#[non_exhaustive]`: further profiles may be added without a breaking change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum HistogramProfile {
    /// Classic Prometheus cumulative `le` buckets (the default).
    #[default]
    Le,
    /// VictoriaMetrics non-cumulative `vmrange` buckets. Exponential histograms
    /// render natively; classic bucket histograms fall back to `le` (they have
    /// no native `vmrange` form).
    VmRange,
}

/// Writes metrics in the OpenMetrics text exposition format.
///
/// Tracks which metric families have already had a `# TYPE` line written, so a
/// family split across multiple label sets (e.g. an error breakdown) is
/// declared exactly once. Call [`OpenMetricsEncoder::finish`] to write the
/// closing `# EOF`.
///
/// This is the OpenMetrics implementation of [`metered::MetricSink`]; a service
/// that wants a different exposition format implements its own sink and the
/// rest of `metered` is unchanged.
pub struct OpenMetricsEncoder<'a> {
    out: &'a mut (dyn Write + 'a),
    declared: HashSet<String>,
    profile: HistogramProfile,
}

impl<'a> OpenMetricsEncoder<'a> {
    /// Creates an encoder writing into `out`, rendering histograms as classic
    /// `le` buckets.
    pub fn new(out: &'a mut (dyn Write + 'a)) -> Self {
        OpenMetricsEncoder {
            out,
            declared: HashSet::new(),
            profile: HistogramProfile::Le,
        }
    }

    /// Sets the histogram bucket rendering profile (e.g.
    /// [`HistogramProfile::VmRange`] for VictoriaMetrics).
    pub fn histogram_profile(mut self, profile: HistogramProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Writes the closing `# EOF` line that terminates an OpenMetrics document.
    pub fn finish(self) -> fmt::Result {
        writeln!(self.out, "# EOF")
    }

    /// Renders a schema and sampled values as OpenMetrics text.
    ///
    /// Family `# HELP` / `# TYPE` / `# UNIT` headers come from the schema (keyed
    /// to the exact family name, so metadata never leaks across composition
    /// boundaries); the samples come from the values. A family's header is
    /// written at most once per encoder, so composing several trees onto one
    /// encoder will not re-declare a shared family. Call
    /// [`OpenMetricsEncoder::finish`] afterwards to write the terminal `# EOF`.
    pub fn encode_document(&mut self, schema: &MetricSchema, values: &MetricValues) -> fmt::Result {
        for family in schema.families() {
            if self.declared.insert(family.name.clone()) {
                write_family_header(&mut *self.out, family)?;
            }
        }
        for sample in values.samples() {
            write_sample(&mut *self.out, sample)?;
        }
        for histogram in values.histograms() {
            let resolved = resolve_profile(self.profile, schema, &histogram.name);
            for sample in histogram_samples(histogram, resolved) {
                write_sample(&mut *self.out, &sample)?;
            }
        }
        Ok(())
    }
}

/// Resolves a histogram family's effective render: the family's declared
/// intent ([`metered::HistogramRender`], from the schema) gated by the
/// document's capability (`profile` — whether the scraper can ingest
/// `vmrange` at all).
///
/// | declared \ document | `Le` (default)  | `VmRange`-capable |
/// |---------------------|-----------------|-------------------|
/// | `Auto`              | `le`            | `vmrange`         |
/// | `Le`                | `le`            | `le`              |
/// | `VmRange`           | `le` (degraded) | `vmrange`         |
pub(crate) fn resolve_profile(
    document: HistogramProfile,
    schema: &MetricSchema,
    family: &str,
) -> HistogramProfile {
    match document {
        // The scraper cannot ingest vmrange: every declaration degrades to le.
        HistogramProfile::Le => HistogramProfile::Le,
        HistogramProfile::VmRange => match schema
            .family(family)
            .map(|family| family.histogram_render)
            .unwrap_or_default()
        {
            metered::HistogramRender::Le => HistogramProfile::Le,
            metered::HistogramRender::Auto | metered::HistogramRender::VmRange => {
                HistogramProfile::VmRange
            }
        },
    }
}

/// Expands a structured histogram into the bucket / `_sum` / `_count` samples
/// for the given profile. Shared by [`OpenMetricsEncoder`] and the incremental
/// [`OpenMetricsRender`](crate::OpenMetricsRender).
pub(crate) fn histogram_samples(
    histogram: &HistogramValue,
    profile: HistogramProfile,
) -> Vec<MetricSample> {
    match (&histogram.data, profile) {
        (HistogramData::Exponential(snapshot), HistogramProfile::VmRange) => {
            vmrange_samples(&histogram.name, &histogram.labels, snapshot)
        }
        (HistogramData::Exponential(snapshot), _) => le_samples(
            &histogram.name,
            &histogram.labels,
            &snapshot.to_histogram_snapshot(),
        ),
        (HistogramData::Classic(snapshot), _) => {
            le_samples(&histogram.name, &histogram.labels, snapshot)
        }
    }
}

fn le_samples(
    name: &str,
    labels: &[(String, String)],
    snapshot: &HistogramSnapshot,
) -> Vec<MetricSample> {
    let bucket_name = format!("{name}_bucket");
    let mut samples = Vec::with_capacity(snapshot.buckets.len() + 2);
    for bucket in &snapshot.buckets {
        let le = if bucket.le.is_infinite() {
            "+Inf".to_owned()
        } else {
            bucket.le.to_string()
        };
        let mut bucket_labels = labels.to_vec();
        bucket_labels.push(("le".to_owned(), le));
        let exemplar = bucket.exemplar.as_ref().map(|exemplar| MetricExemplar {
            labels: exemplar.labels.clone(),
            value: exemplar.value,
            timestamp: exemplar.timestamp_seconds,
        });
        samples.push(MetricSample {
            name: bucket_name.clone(),
            labels: bucket_labels,
            value: MetricSampleValue::from(bucket.cumulative_count),
            exemplar,
        });
    }
    push_sum_count(&mut samples, name, labels, snapshot.sum, snapshot.count);
    samples
}

fn vmrange_samples(
    name: &str,
    labels: &[(String, String)],
    snapshot: &ExponentialSnapshot,
) -> Vec<MetricSample> {
    let bucket_name = format!("{name}_bucket");
    let mut samples = Vec::with_capacity(snapshot.buckets.len() + 2);
    for bucket in &snapshot.buckets {
        // VictoriaMetrics reads the `lo...hi` edges from the label; the counts
        // are per-bucket (not cumulative).
        let vmrange = format!("{:.3e}...{:.3e}", bucket.lower, bucket.upper);
        let mut bucket_labels = labels.to_vec();
        bucket_labels.push(("vmrange".to_owned(), vmrange));
        // Exemplar parity with the `le` render: the sampled per-bucket
        // exemplar rides the bucket sample in either encoding.
        let exemplar = bucket.exemplar.as_ref().map(|exemplar| MetricExemplar {
            labels: exemplar.labels.clone(),
            value: exemplar.value,
            timestamp: exemplar.timestamp_seconds,
        });
        samples.push(MetricSample {
            name: bucket_name.clone(),
            labels: bucket_labels,
            value: MetricSampleValue::from(bucket.count),
            exemplar,
        });
    }
    push_sum_count(&mut samples, name, labels, snapshot.sum, snapshot.count);
    samples
}

fn push_sum_count(
    samples: &mut Vec<MetricSample>,
    name: &str,
    labels: &[(String, String)],
    sum: f64,
    count: u64,
) {
    samples.push(MetricSample {
        name: format!("{name}_sum"),
        labels: labels.to_vec(),
        value: MetricSampleValue::from(sum),
        exemplar: None,
    });
    samples.push(MetricSample {
        name: format!("{name}_count"),
        labels: labels.to_vec(),
        value: MetricSampleValue::from(count),
        exemplar: None,
    });
}

impl MetricSink for OpenMetricsEncoder<'_> {
    fn encode_document(&mut self, schema: &MetricSchema, values: &MetricValues) -> fmt::Result {
        OpenMetricsEncoder::encode_document(self, schema, values)
    }
}

/// Writes a family's `# HELP` / `# TYPE` / `# UNIT` header lines. The single
/// header writer shared by [`OpenMetricsEncoder::encode_document`] and the
/// incremental [`OpenMetricsRender`](crate::OpenMetricsRender).
pub(crate) fn write_family_header(out: &mut dyn Write, family: &MetricFamilySchema) -> fmt::Result {
    if let Some(help) = &family.help {
        writeln!(out, "# HELP {} {}", family.name, escape_help(help.as_str()))?;
    }
    writeln!(
        out,
        "# TYPE {} {}",
        family.name,
        family.metric_type.as_str()
    )?;
    if let Some(unit) = &family.unit {
        // OpenMetrics 1.0 requires a declared unit to be a suffix of the family
        // name -- either the whole name (`seconds` + `seconds`) or an
        // `_`-separated tail (`foo_seconds` + `seconds`). Prometheus's
        // OpenMetrics parser (the `application/openmetrics-text; version=1.0.0`
        // reader) rejects the ENTIRE scrape when this is violated, so one
        // mislabeled unit would drop every metric in the document. Emit the
        // `# UNIT` line only when the final, prefixed name conforms; otherwise
        // drop just that metadata line and keep the family renderable. We never
        // panic on this path: a naming mistake must not abort the exposition
        // (same reason `Gauge::decr` saturates instead of asserting).
        if unit_is_conformant(&family.name, unit.as_str()) {
            writeln!(out, "# UNIT {} {}", family.name, unit.as_str())?;
        } else {
            #[cfg(debug_assertions)]
            eprintln!(
                "metered-om: dropping non-conformant `# UNIT {name} {unit}` \
                 (OpenMetrics requires the unit to be the family name or an \
                 `_`-separated suffix of it; rename the family to `{name}_{unit}` \
                 to emit it and keep Prometheus from rejecting the scrape)",
                name = family.name,
                unit = unit.as_str(),
            );
        }
    }
    Ok(())
}

/// Whether `unit` may be written as the `# UNIT` for family `name` under the
/// OpenMetrics unit-suffix rule: an empty unit is always allowed, and otherwise
/// the unit must be the whole name or an `_`-separated suffix of it. This mirrors
/// the check Prometheus's OpenMetrics parser applies before accepting a scrape,
/// so a family we render is one Prometheus will not reject.
fn unit_is_conformant(name: &str, unit: &str) -> bool {
    if unit.is_empty() || name == unit {
        return true;
    }
    name.len() > unit.len()
        && name.ends_with(unit)
        && name.as_bytes()[name.len() - unit.len() - 1] == b'_'
}

/// Writes one sample line, including an optional trailing exemplar. The single
/// sample writer shared by the eager and incremental renderers.
pub(crate) fn write_sample(out: &mut dyn Write, sample: &MetricSample) -> fmt::Result {
    write!(
        out,
        "{}{} {}",
        sample.name,
        format_owned_labels(&sample.labels),
        sample.value
    )?;
    if let Some(exemplar) = &sample.exemplar {
        write!(
            out,
            " # {} {}",
            format_owned_labels(&exemplar.labels),
            exemplar.value
        )?;
        if let Some(timestamp) = exemplar.timestamp {
            write!(out, " {timestamp}")?;
        }
    }
    writeln!(out)
}

/// Formats a Prometheus label block, e.g. `{le="0.005",service="x"}`, or the
/// empty string when there are no labels.
fn format_labels(labels: &[(&str, &str)]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let mut out = String::from("{");
    for (i, (k, v)) in labels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{k}=\"{}\"", EscapedLabel(v));
    }
    out.push('}');
    out
}

fn format_owned_labels(labels: &[(String, String)]) -> String {
    let borrowed: Vec<(&str, &str)> = labels
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    format_labels(&borrowed)
}

/// Escapes `HELP` text per OpenMetrics: backslash and newline only.
fn escape_help(help: &str) -> String {
    let mut out = String::with_capacity(help.len());
    for ch in help.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

struct EscapedLabel<'a>(&'a str);

impl Display for EscapedLabel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for ch in self.0.chars() {
            match ch {
                '\\' => f.write_str("\\\\")?,
                '"' => f.write_str("\\\"")?,
                '\n' => f.write_str("\\n")?,
                other => f.write_char(other)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metered::bucket_histogram::{BucketHistogram, Buckets, Exemplar, HistogramSnapshot};
    use metered::{Metric, MetricTree, MetricType};
    use metered_semantic::{ErrorCount, HitCount, InFlight, NoneCount};

    #[test]
    fn counter_and_gauge_encode_with_type_and_labels() {
        let hits: HitCount = HitCount::default();
        hits.incr();
        hits.incr();
        let inflight: InFlight = InFlight::default();
        inflight.incr();

        let mut buf = String::new();
        {
            let mut enc = OpenMetricsEncoder::new(&mut buf);
            hits.encode("requests", &[("svc", "x")], &mut enc).unwrap();
            inflight
                .encode("active", &[("svc", "x")], &mut enc)
                .unwrap();
            enc.finish().unwrap();
        }
        assert!(buf.contains("# TYPE requests counter"));
        assert!(buf.contains("requests_total{svc=\"x\"} 2"));
        assert!(buf.contains("# TYPE active gauge"));
        assert!(buf.contains("active{svc=\"x\"} 1"));
        assert!(buf.trim_end().ends_with("# EOF"));
    }

    #[test]
    fn bucket_histogram_encodes_buckets_and_exemplar() {
        let h = BucketHistogram::new(Buckets::custom([0.005, 0.01]));
        h.observe(0.003);
        h.observe_with_exemplar(
            0.008,
            Exemplar {
                labels: vec![("trace_id".into(), "deadbeef".into())],
                value: 0.008,
                timestamp_seconds: None,
            },
        );

        let mut buf = String::new();
        {
            let mut enc = OpenMetricsEncoder::new(&mut buf);
            h.encode("call_duration_seconds", &[("service", "api")], &mut enc)
                .unwrap();
            enc.finish().unwrap();
        }
        assert!(buf.contains("# TYPE call_duration_seconds histogram"));
        assert!(buf.contains("call_duration_seconds_bucket{service=\"api\",le=\"0.005\"} 1"));
        assert!(buf.contains("call_duration_seconds_bucket{service=\"api\",le=\"+Inf\"} 2"));
        assert!(buf.contains("call_duration_seconds_count{service=\"api\"} 2"));
        assert!(buf.contains("# {trace_id=\"deadbeef\"} 0.008"));
    }

    #[test]
    fn info_and_stateset_encode() {
        use metered::{InfoMetric, StateSet};

        let info = InfoMetric::new([("version", "1.2.3")]);
        let state = StateSet::new(["starting", "running", "stopped"]);
        state.set("running");

        let mut buf = String::new();
        {
            let mut enc = OpenMetricsEncoder::new(&mut buf);
            info.encode("build", &[], &mut enc).unwrap();
            state.encode("process_state", &[], &mut enc).unwrap();
            enc.finish().unwrap();
        }
        assert!(buf.contains("# TYPE build info"));
        assert!(buf.contains("build_info{version=\"1.2.3\"} 1"));
        assert!(buf.contains("# TYPE process_state stateset"));
        assert!(buf.contains("process_state{process_state=\"starting\"} 0"));
        assert!(buf.contains("process_state{process_state=\"running\"} 1"));
    }

    fn render(schema: &MetricSchema, values: &MetricValues) -> String {
        let mut buf = String::new();
        {
            let mut enc = OpenMetricsEncoder::new(&mut buf);
            enc.encode_document(schema, values).unwrap();
            enc.finish().unwrap();
        }
        buf
    }

    #[test]
    fn type_line_is_written_once_per_family() {
        // One family, two label sets (as an error breakdown produces).
        let mut schema = MetricSchema::new();
        schema.add_family("errors", MetricType::Counter, &[("kind", "")]);
        let mut values = MetricValues::new();
        values.counter("errors", &[("kind", "foo")], 3u64);
        values.counter("errors", &[("kind", "bar")], 1u64);

        let buf = render(&schema, &values);
        assert_eq!(buf.matches("# TYPE errors counter").count(), 1);
        assert!(buf.contains("errors_total{kind=\"foo\"} 3"));
        assert!(buf.contains("errors_total{kind=\"bar\"} 1"));
    }

    #[test]
    fn help_text_is_escaped_and_metadata_is_exact() {
        let mut schema = MetricSchema::new();
        schema.set_metadata_for(
            "requests",
            Some(metered::Help::from("line 1\nline \\2")),
            Some(metered::Unit::Requests),
        );
        // Metadata for a family that is never added must not leak into output.
        schema.set_help_for("requests_child", "wrong");
        schema.add_family("requests", MetricType::Counter, &[]);
        let mut values = MetricValues::new();
        values.counter("requests", &[], 1u64);

        let buf = render(&schema, &values);
        assert!(buf.contains("# HELP requests line 1\\nline \\\\2"));
        assert!(buf.contains("# UNIT requests requests"));
        assert!(!buf.contains("wrong"));
    }

    #[test]
    fn unit_line_is_emitted_only_for_a_conformant_suffix() {
        // (a) A `_seconds` suffix conforms, so the `# UNIT` line is emitted.
        let mut schema = MetricSchema::new();
        schema.set_unit_for("call_duration_seconds", metered::Unit::Seconds);
        schema.add_family("call_duration_seconds", MetricType::Histogram, &[]);
        let mut values = MetricValues::new();
        values.sample("call_duration_seconds_count", &[], 0u64);

        let buf = render(&schema, &values);
        assert!(buf.contains("# TYPE call_duration_seconds histogram"));
        assert!(buf.contains("# UNIT call_duration_seconds seconds"));

        // (b) `items` is not a suffix of `queue_depth`, so no `# UNIT` line is
        // written -- Prometheus would reject the whole scrape otherwise -- but
        // the family still renders its `# TYPE` and sample.
        let mut schema = MetricSchema::new();
        schema.set_unit_for("queue_depth", metered::Unit::Items);
        schema.add_family("queue_depth", MetricType::Gauge, &[]);
        let mut values = MetricValues::new();
        values.gauge("queue_depth", &[], 5);

        let buf = render(&schema, &values);
        assert!(
            !buf.contains("# UNIT queue_depth"),
            "non-conformant unit suppressed"
        );
        assert!(buf.contains("# TYPE queue_depth gauge"));
        assert!(buf.contains("queue_depth 5"));
    }

    #[test]
    fn unit_conformance_matches_the_openmetrics_suffix_rule() {
        // Whole-name match, `_`-separated suffix, and the empty unit all pass.
        assert!(unit_is_conformant("seconds", "seconds"));
        assert!(unit_is_conformant("call_duration_seconds", "seconds"));
        assert!(unit_is_conformant("app_requests", "requests"));
        assert!(unit_is_conformant("queue_depth", ""));
        // A bare suffix without the `_` separator, or no suffix at all, fails.
        assert!(!unit_is_conformant("queue_depth", "items"));
        assert!(!unit_is_conformant("latencyseconds", "seconds"));
    }

    #[test]
    fn summary_samples_render_quantiles_sum_and_count() {
        let mut schema = MetricSchema::new();
        schema.add_family("legacy_latency", MetricType::Summary, &[("quantile", "")]);
        let mut values = MetricValues::new();
        values.sample("legacy_latency", &[("quantile", "0.95")], 42.0);
        values.sample("legacy_latency_sum", &[], 100.0);
        values.sample("legacy_latency_count", &[], 7u64);

        let buf = render(&schema, &values);
        assert!(buf.contains("# TYPE legacy_latency summary"));
        assert!(buf.contains("legacy_latency{quantile=\"0.95\"} 42"));
        assert!(buf.contains("legacy_latency_sum 100"));
        assert!(buf.contains("legacy_latency_count 7"));
    }

    #[test]
    fn label_values_escape_quotes_backslashes_and_newlines() {
        let mut schema = MetricSchema::new();
        schema.add_family("weird", MetricType::Gauge, &[("label", "")]);
        let mut values = MetricValues::new();
        values.gauge("weird", &[("label", "a\"b\\c\n")], 1);

        let buf = render(&schema, &values);
        assert!(buf.contains("weird{label=\"a\\\"b\\\\c\\n\"} 1"));
    }

    #[test]
    fn encode_document_renders_schema_metadata_and_values() {
        let mut schema = MetricSchema::new();
        schema.set_metadata_for(
            "requests",
            Some(metered::Help::from("Total requests")),
            Some(metered::Unit::Requests),
        );
        schema.add_family("requests", MetricType::Counter, &[("service", "api")]);

        let mut values = MetricValues::new();
        values.counter("requests", &[("service", "api")], 3u64);

        let buf = render(&schema, &values);
        assert!(buf.contains("# HELP requests Total requests"));
        assert!(buf.contains("# TYPE requests counter"));
        assert!(buf.contains("# UNIT requests requests"));
        assert!(buf.contains("requests_total{service=\"api\"} 3"));
    }

    #[test]
    fn metric_values_render_histogram_exemplars() {
        let mut values = MetricValues::new();
        let exemplar = Exemplar {
            labels: vec![("trace_id".to_owned(), "abc".to_owned())],
            value: 0.5,
            timestamp_seconds: Some(12.0),
        };
        let snapshot = HistogramSnapshot {
            buckets: vec![metered::bucket_histogram::Bucket {
                le: f64::INFINITY,
                cumulative_count: 1,
                exemplar: Some(exemplar),
            }],
            sum: 0.5,
            count: 1,
        };
        values.histogram("latency_seconds", &[], &snapshot);

        let mut schema = MetricSchema::new();
        schema.add_family("latency_seconds", MetricType::Histogram, &[]);

        let buf = render(&schema, &values);
        assert!(buf.contains("latency_seconds_bucket{le=\"+Inf\"} 1 # {trace_id=\"abc\"} 0.5 12"));
        assert!(buf.contains("latency_seconds_sum 0.5"));
        assert!(buf.contains("latency_seconds_count 1"));
    }

    #[test]
    fn leaf_metrics_collect_values_from_one_source_of_truth() {
        use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};

        let enabled = AtomicBool::new(true);
        let signed = AtomicI64::new(-4);
        let depth = AtomicUsize::new(9);
        let counter = AtomicU64::new(0);
        metered::Counter::incr_by(&counter, 3);
        let gauge = AtomicI64::new(0);
        metered::Gauge::set(&gauge, -2);
        let hit: HitCount = HitCount::default();
        hit.incr();
        let errors: ErrorCount = ErrorCount::default();
        errors.incr();
        let none: NoneCount = NoneCount::default();
        none.incr();
        let inflight: InFlight = InFlight::default();
        inflight.incr();

        let mut values = MetricValues::new();
        enabled.collect_metric("enabled", &[], &mut values);
        signed.collect_metric("signed", &[], &mut values);
        depth.collect_metric("depth", &[], &mut values);
        counter.collect_metric("requests", &[], &mut values);
        gauge.collect_metric("queue_depth", &[], &mut values);
        hit.collect_metric("hits", &[], &mut values);
        errors.collect_metric("errors", &[], &mut values);
        none.collect_metric("none", &[], &mut values);
        inflight.collect_metric("in_flight", &[], &mut values);

        let samples = values.samples();
        let value_of = |name: &str| {
            samples
                .iter()
                .find(|sample| sample.name == name)
                .map(|sample| sample.value.to_string())
        };
        assert_eq!(value_of("enabled").as_deref(), Some("1"));
        assert_eq!(value_of("signed").as_deref(), Some("-4"));
        assert_eq!(value_of("depth").as_deref(), Some("9"));
        assert_eq!(value_of("requests_total").as_deref(), Some("3"));
        assert_eq!(value_of("queue_depth").as_deref(), Some("-2"));
        assert_eq!(value_of("hits_total").as_deref(), Some("1"));
        assert_eq!(value_of("errors_total").as_deref(), Some("1"));
        assert_eq!(value_of("none_total").as_deref(), Some("1"));
        assert_eq!(value_of("in_flight").as_deref(), Some("1"));

        enabled.store(false, Ordering::Relaxed);
        assert_eq!(enabled.metric_type(), MetricType::Gauge);
        assert_eq!(counter.metric_type(), MetricType::Counter);
    }

    #[test]
    fn atomic_leaf_metrics_encode_through_metric_tree_default() {
        use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize};

        let enabled = AtomicBool::new(true);
        let signed = AtomicI64::new(-8);
        let depth = AtomicUsize::new(12);

        let mut buf = String::new();
        {
            let mut enc = OpenMetricsEncoder::new(&mut buf);
            enabled.encode("enabled", &[], &mut enc).unwrap();
            signed.encode("signed", &[], &mut enc).unwrap();
            depth.encode("depth", &[], &mut enc).unwrap();
            enc.finish().unwrap();
        }

        assert!(buf.contains("# TYPE enabled gauge"));
        assert!(buf.contains("enabled 1"));
        assert!(buf.contains("signed -8"));
        assert!(buf.contains("depth 12"));
    }

    #[test]
    fn encode_document_renders_exemplar_without_timestamp() {
        let mut schema = MetricSchema::new();
        schema.add_family("latency", MetricType::Histogram, &[]);

        let mut values = MetricValues::new();
        let exemplar = Exemplar {
            labels: vec![("trace_id".to_owned(), "abc".to_owned())],
            value: 1.5,
            timestamp_seconds: None,
        };
        values.sample_with_exemplar("latency_bucket", &[("le", "+Inf")], 1, &exemplar);

        let mut buf = String::new();
        {
            let mut enc = OpenMetricsEncoder::new(&mut buf);
            enc.encode_document(&schema, &values).unwrap();
            enc.finish().unwrap();
        }

        assert!(buf.contains("latency_bucket{le=\"+Inf\"} 1 # {trace_id=\"abc\"} 1.5"));
    }
}
