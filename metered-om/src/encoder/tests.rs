use super::*;
use metered::bucket_histogram::{BucketHistogram, Buckets, Exemplar, HistogramSnapshot};
use metered::{Metric, MetricTree, MetricType};

#[test]
fn counter_and_gauge_encode_with_type_and_labels() {
    use std::sync::atomic::{AtomicI64, AtomicU64};

    let hits = AtomicU64::new(0);
    metered::Counter::incr(&hits);
    metered::Counter::incr(&hits);
    let inflight = AtomicI64::new(0);
    metered::Gauge::incr(&inflight);

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
    // The full document, line for line: every bucket count, the exemplar on
    // the bucket that observed it, and the exact `_sum` / `_count` values --
    // so a zeroed or shifted observation cannot slip past this test.
    assert_eq!(
        buf,
        "# TYPE call_duration_seconds histogram\n\
         call_duration_seconds_bucket{service=\"api\",le=\"0.005\"} 1\n\
         call_duration_seconds_bucket{service=\"api\",le=\"0.01\"} 2 # {trace_id=\"deadbeef\"} 0.008\n\
         call_duration_seconds_bucket{service=\"api\",le=\"+Inf\"} 2\n\
         call_duration_seconds_sum{service=\"api\"} 0.011\n\
         call_duration_seconds_count{service=\"api\"} 2\n\
         # EOF\n"
    );
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

/// The family a rendered line belongs to, per the same suffix rules the
/// grouping uses -- for asserting the OpenMetrics grouping MUST below.
fn family_of_line(line: &str) -> Option<String> {
    if line == "# EOF" {
        return None;
    }
    for directive in ["# HELP ", "# TYPE ", "# UNIT "] {
        if let Some(rest) = line.strip_prefix(directive) {
            return rest.split_whitespace().next().map(str::to_owned);
        }
    }
    let name_end = line.find(['{', ' ']).unwrap_or(line.len());
    let name = &line[..name_end];
    let base = FAMILY_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name);
    Some(base.to_owned())
}

/// Asserts every family's lines form one contiguous run.
fn assert_family_grouping(text: &str) {
    let mut seen: Vec<String> = Vec::new();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let Some(family) = family_of_line(line) else {
            continue;
        };
        if seen.last() != Some(&family) {
            assert!(
                !seen.contains(&family),
                "family `{family}` is interleaved with another family:\n{text}"
            );
            seen.push(family);
        }
    }
}

#[test]
fn families_are_not_interleaved() {
    // Two scalar families around a histogram family: OpenMetrics requires
    // all lines of a MetricFamily as one uninterrupted group, so each
    // header must be immediately followed by that family's samples --
    // including the histogram's expanded bucket/_sum/_count lines, which
    // used to trail the whole document.
    let mut schema = MetricSchema::new();
    schema.add_family("alpha", MetricType::Counter, &[]);
    schema.add_family("latency_seconds", MetricType::Histogram, &[]);
    schema.add_family("omega", MetricType::Gauge, &[]);

    let mut values = MetricValues::new();
    values.counter("alpha", &[], 1u64);
    values.gauge("omega", &[], 5);
    let snapshot = HistogramSnapshot::new(
        vec![metered::bucket_histogram::Bucket::new(
            f64::INFINITY,
            1,
            None,
        )],
        0.5,
        1,
    );
    values.histogram("latency_seconds", &[], &snapshot);

    let buf = render(&schema, &values);
    assert_family_grouping(&buf);
    // Spot-check the shape: the histogram's samples sit right under its
    // header, before the next family's header.
    let type_line = buf
        .lines()
        .position(|line| line == "# TYPE latency_seconds histogram")
        .expect("histogram TYPE line");
    let lines: Vec<&str> = buf.lines().collect();
    assert!(lines[type_line + 1].starts_with("latency_seconds_bucket{"));
    assert_eq!(lines[type_line + 2], "latency_seconds_sum 0.5");
    assert_eq!(lines[type_line + 3], "latency_seconds_count 1");
}

#[test]
fn undeclared_passthrough_samples_group_by_inferred_family() {
    // Foreign (undeclared) samples -- the TextSourceTree shape -- still
    // group per inferred family, after the declared families.
    let mut schema = MetricSchema::new();
    schema.add_family("native", MetricType::Counter, &[]);
    let mut values = MetricValues::new();
    values.sample("legacy_latency", &[("quantile", "0.99")], 250.5);
    values.counter("native", &[], 1u64);
    values.sample("legacy_latency_sum", &[], 1000.0);
    values.sample("legacy_latency_count", &[], 7u64);

    let buf = render(&schema, &values);
    assert_family_grouping(&buf);
    assert!(buf.contains("legacy_latency_count 7"));
    assert!(buf.contains("native_total 1"));
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
fn a_family_may_continue_across_adjacent_encode_calls() {
    // Composing two trees that share a family: the second call continues the
    // family written last, declared once, still one contiguous group.
    let mut schema = MetricSchema::new();
    schema.add_family("errors", MetricType::Counter, &[("kind", "")]);
    let mut foo = MetricValues::new();
    foo.counter("errors", &[("kind", "foo")], 3u64);
    let mut bar = MetricValues::new();
    bar.counter("errors", &[("kind", "bar")], 1u64);

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        enc.encode_document(&schema, &foo).unwrap();
        enc.encode_document(&schema, &bar).unwrap();
        enc.finish().unwrap();
    }
    assert_eq!(
        buf,
        "# TYPE errors counter\n\
         errors_total{kind=\"foo\"} 3\n\
         errors_total{kind=\"bar\"} 1\n\
         # EOF\n"
    );
}

#[test]
fn resuming_a_family_after_another_family_is_a_clean_error() {
    use std::error::Error as _;

    // A, B, A across three calls: resuming `alpha` after `omega` was written
    // would break the OpenMetrics contiguity MUST, so the encoder must refuse
    // with an error rather than emit an invalid document.
    let document = |name: &str, value: u64| {
        let mut schema = MetricSchema::new();
        schema.add_family(name, MetricType::Counter, &[]);
        let mut values = MetricValues::new();
        values.counter(name, &[], value);
        (schema, values)
    };
    let (alpha_schema, alpha) = document("alpha", 1);
    let (omega_schema, omega) = document("omega", 2);

    let mut buf = String::new();
    let mut enc = OpenMetricsEncoder::new(&mut buf);
    enc.encode_document(&alpha_schema, &alpha).unwrap();
    enc.encode_document(&omega_schema, &omega).unwrap();
    let error = enc.encode_document(&alpha_schema, &alpha).unwrap_err();
    let cause = error
        .source()
        .expect("the error names the family")
        .to_string();
    assert!(
        cause.contains("alpha"),
        "cause must name the family: {cause}"
    );

    // Nothing of the refused family was appended: the document holds exactly
    // the two valid calls.
    assert_eq!(
        buf,
        "# TYPE alpha counter\n\
         alpha_total 1\n\
         # TYPE omega counter\n\
         omega_total 2\n"
    );
}

#[test]
fn a_family_from_earlier_in_the_same_call_cannot_be_resumed_later() {
    // Call 1 emits alpha then omega; a later call for alpha alone is A, B, A
    // even though alpha was the FIRST family of its document.
    let mut schema = MetricSchema::new();
    schema.add_family("alpha", MetricType::Counter, &[]);
    schema.add_family("omega", MetricType::Gauge, &[]);
    let mut values = MetricValues::new();
    values.counter("alpha", &[], 1u64);
    values.gauge("omega", &[], 5);

    let mut alpha_schema = MetricSchema::new();
    alpha_schema.add_family("alpha", MetricType::Counter, &[]);
    let mut alpha = MetricValues::new();
    alpha.counter("alpha", &[], 2u64);

    let mut buf = String::new();
    let mut enc = OpenMetricsEncoder::new(&mut buf);
    enc.encode_document(&schema, &values).unwrap();
    enc.encode_document(&alpha_schema, &alpha).unwrap_err();
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
fn resolve_profile_pins_the_declared_by_document_matrix() {
    use metered::HistogramRender;

    let schema_declaring = |render: HistogramRender| {
        let mut schema = MetricSchema::new();
        schema.set_render_for("latency_seconds", render);
        schema.add_family("latency_seconds", MetricType::Histogram, &[]);
        schema
    };

    // An `le`-only document degrades every declaration to `le`.
    for render in [
        HistogramRender::Auto,
        HistogramRender::Le,
        HistogramRender::VmRange,
    ] {
        assert_eq!(
            resolve_profile(
                HistogramProfile::Le,
                schema_declaring(render).family("latency_seconds")
            ),
            HistogramProfile::Le,
            "{render:?} must degrade to le in an le document"
        );
    }

    // A vmrange-capable document renders the declared-or-default native
    // form, except an explicit `Le` declaration.
    let vm = HistogramProfile::VmRange;
    let resolved = |render| resolve_profile(vm, schema_declaring(render).family("latency_seconds"));
    assert_eq!(resolved(HistogramRender::Auto), HistogramProfile::VmRange);
    assert_eq!(resolved(HistogramRender::Le), HistogramProfile::Le);
    assert_eq!(
        resolved(HistogramRender::VmRange),
        HistogramProfile::VmRange
    );

    // An undeclared family (no group header) defaults to `Auto` (native form).
    assert_eq!(resolve_profile(vm, None), HistogramProfile::VmRange);
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
    let snapshot = HistogramSnapshot::new(
        vec![metered::bucket_histogram::Bucket::new(
            f64::INFINITY,
            1,
            Some(exemplar),
        )],
        0.5,
        1,
    );
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

    let mut values = MetricValues::new();
    enabled.collect_metric("enabled", &[], &mut values);
    signed.collect_metric("signed", &[], &mut values);
    depth.collect_metric("depth", &[], &mut values);
    counter.collect_metric("requests", &[], &mut values);
    gauge.collect_metric("queue_depth", &[], &mut values);

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
fn vmrange_render_represents_zero_and_overflow_observations() {
    use metered::exponential_histogram::{ExponentialBucket, ExponentialSnapshot};

    // 2 non-positive + 3 in-range + 4 above-range observations: every one
    // must land in exactly one vmrange bucket so the per-bucket counts sum
    // to `_count` (VictoriaMetrics quantiles work off the buckets alone).
    let snapshot = ExponentialSnapshot::new(
        0,
        2,
        vec![ExponentialBucket::new(0, 1.0, 2.0, 3, None)],
        4,
        100.0,
        9,
    );
    let mut schema = MetricSchema::new();
    schema.add_family("latency_seconds", MetricType::Histogram, &[]);
    let mut values = MetricValues::new();
    values.exponential_histogram("latency_seconds", &[], &snapshot);

    let mut buf = String::new();
    {
        let mut enc =
            OpenMetricsEncoder::new(&mut buf).histogram_profile(HistogramProfile::VmRange);
        enc.encode_document(&schema, &values).unwrap();
        enc.finish().unwrap();
    }

    assert!(
        buf.contains("latency_seconds_bucket{vmrange=\"0...0\"} 2"),
        "zero bucket rendered:\n{buf}"
    );
    assert!(buf.contains("latency_seconds_bucket{vmrange=\"1.000e0...2.000e0\"} 3"));
    assert!(
        buf.contains("latency_seconds_bucket{vmrange=\"2.000e0...+Inf\"} 4"),
        "overflow bucket rendered:\n{buf}"
    );
    // Exact `_sum` / `_count` lines: a zeroed or shifted observation set
    // cannot slip past a supplied-but-unasserted sum.
    assert!(
        buf.contains("latency_seconds_sum 100\n"),
        "exact _sum rendered:\n{buf}"
    );
    assert!(
        buf.contains("latency_seconds_count 9\n"),
        "exact _count rendered:\n{buf}"
    );

    let bucket_total: u64 = buf
        .lines()
        .filter(|line| line.starts_with("latency_seconds_bucket{"))
        .filter_map(|line| line.rsplit(' ').next())
        .filter_map(|count| count.parse::<u64>().ok())
        .sum();
    assert_eq!(bucket_total, 9, "bucket counts must sum to _count:\n{buf}");
}

#[test]
fn vmrange_render_omits_edge_buckets_when_unpopulated() {
    use metered::DynamicExponentialHistogram;

    let histogram = DynamicExponentialHistogram::with_params(5, 256);
    histogram.observe(0.25);

    let mut schema = MetricSchema::new();
    schema.add_family("clean_seconds", MetricType::Histogram, &[]);
    let mut values = MetricValues::new();
    values.exponential_histogram("clean_seconds", &[], &histogram.snapshot());

    let mut buf = String::new();
    {
        let mut enc =
            OpenMetricsEncoder::new(&mut buf).histogram_profile(HistogramProfile::VmRange);
        enc.encode_document(&schema, &values).unwrap();
        enc.finish().unwrap();
    }
    assert!(!buf.contains("vmrange=\"0...0\""));
    assert!(!buf.contains("...+Inf"));
}

#[test]
fn empty_exemplar_labelset_renders_the_mandatory_braces() {
    let mut schema = MetricSchema::new();
    schema.add_family("latency", MetricType::Histogram, &[]);
    let mut values = MetricValues::new();
    let exemplar = Exemplar {
        labels: Vec::new(),
        value: 1.5,
        timestamp_seconds: None,
    };
    values.sample_with_exemplar("latency_bucket", &[("le", "+Inf")], 1, &exemplar);

    let buf = render(&schema, &values);
    // The exemplar grammar requires `{}` even for an empty labelset.
    assert!(
        buf.contains("latency_bucket{le=\"+Inf\"} 1 # {} 1.5"),
        "empty exemplar labelset must render as `# {{}} value`:\n{buf}"
    );
    // And the strict parser round-trips the document.
    let doc = crate::OpenMetricsDocument::parse(&buf).unwrap();
    let parsed = doc.sample("latency_bucket").unwrap().exemplar.as_ref();
    assert_eq!(parsed.unwrap().value, "1.5");
}

#[test]
fn oversized_exemplar_labelset_is_dropped_not_emitted() {
    let render_with_trace = |trace: String| {
        let mut schema = MetricSchema::new();
        schema.add_family("latency", MetricType::Histogram, &[]);
        let mut values = MetricValues::new();
        let exemplar = Exemplar {
            labels: vec![("trace_id".to_owned(), trace)],
            value: 1.5,
            timestamp_seconds: None,
        };
        values.sample_with_exemplar("latency_bucket", &[("le", "+Inf")], 1, &exemplar);
        render(&schema, &values)
    };

    // `trace_id` is 8 characters: a 120-character value makes exactly the
    // 128-character limit, which the spec still allows.
    let at_limit = render_with_trace("x".repeat(120));
    assert!(at_limit.contains(" # {trace_id="));

    // One character over: the exemplar is dropped (emitting it would make
    // a conformant scraper reject the whole exposition), the sample stays.
    let over_limit = render_with_trace("x".repeat(121));
    assert!(
        !over_limit.contains(" # "),
        "oversized exemplar must be dropped:\n{over_limit}"
    );
    assert!(over_limit.contains("latency_bucket{le=\"+Inf\"} 1\n"));
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
