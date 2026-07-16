use metered::{Buckets, Exemplar, Registry};
use metered_om::OpenMetricsDocument;
use metered_om::OpenMetricsRegistryExt;
use metered_semantic::{Elapsed, ElapsedConfig};
use std::sync::atomic::AtomicU64;

#[test]
fn parses_metadata_samples_labels_exemplars_and_eof() {
    let text = concat!(
        "# HELP requests Total requests\\nfrom tests\n",
        "# TYPE requests counter\n",
        "# UNIT requests requests\n",
        "requests_total{service=\"api\",path=\"a\\\"b\\\\c\\n\"} 42 # {trace_id=\"abc\",span_id=\"def\"} 42 123.5\n",
        "# EOF\n",
    );

    let doc = OpenMetricsDocument::parse(text).expect("parse document");

    let requests = doc.family("requests").expect("requests family");
    assert_eq!(requests.help.as_deref(), Some("Total requests\nfrom tests"));
    assert_eq!(requests.metric_type, Some(metered::MetricType::Counter));
    assert_eq!(requests.unit.as_deref(), Some("requests"));
    assert!(doc.has_eof);

    let sample = doc.sample("requests_total").expect("requests_total sample");
    assert_eq!(sample.value, "42");
    assert_eq!(sample.label("service"), Some("api"));
    assert_eq!(sample.label("path"), Some("a\"b\\c\n"));
    let exemplar = sample.exemplar.as_ref().expect("exemplar");
    assert_eq!(exemplar.label("trace_id"), Some("abc"));
    assert_eq!(exemplar.value, "42");
    assert_eq!(exemplar.timestamp.as_deref(), Some("123.5"));
}

#[test]
fn parses_encoder_output_structurally() {
    let requests = AtomicU64::new(0);
    metered::Counter::incr(&requests);
    let elapsed = Elapsed::with_config(ElapsedConfig {
        buckets: Buckets::custom([1.0]),
        exemplar_source: FixedExemplar,
    });
    metered_semantic::measure!(&elapsed, {});

    let mut registry = Registry::with_prefix("demo");
    registry.label("service", "api");
    registry.register(
        metered::entry::metric("requests")
            .source(&requests)
            .help("Total requests"),
    );
    registry.register(
        metered::entry::metric("duration_seconds")
            .source(&elapsed)
            .help("Duration")
            .unit("seconds"),
    );

    let text = registry.encode_to_string().expect("encode registry");
    let doc = OpenMetricsDocument::parse(&text).expect("parse encoded registry");

    assert_eq!(
        doc.family("demo_requests").unwrap().metric_type,
        Some(metered::MetricType::Counter)
    );
    assert_eq!(
        doc.family("demo_duration_seconds").unwrap().unit.as_deref(),
        Some("seconds")
    );
    assert_eq!(
        doc.sample("demo_requests_total").unwrap().label("service"),
        Some("api")
    );
    assert!(doc
        .samples_named("demo_duration_seconds_bucket")
        .iter()
        .any(|sample| sample.exemplar.is_some()));
}

#[derive(Clone)]
struct FixedExemplar;

impl metered::ExemplarSource for FixedExemplar {
    fn exemplar(&self) -> Option<Exemplar> {
        Some(Exemplar {
            labels: vec![("trace_id".to_owned(), "trace".to_owned())],
            value: 0.0,
            timestamp_seconds: None,
        })
    }
}

#[test]
fn parse_rejects_invalid_label_blocks() {
    let err = OpenMetricsDocument::parse("x{broken} 1\n# EOF\n").unwrap_err();
    assert!(err.to_string().contains("label"));
}

#[test]
fn parser_accepts_plain_samples_without_metadata() {
    // A bare sample line with no preceding `# TYPE`/`# HELP` is valid.
    let doc = OpenMetricsDocument::parse("plain 7\n# EOF\n").unwrap();
    assert_eq!(doc.sample("plain").unwrap().value, "7");
}

#[test]
fn parser_rejects_bad_metadata_and_unknown_types() {
    let err = OpenMetricsDocument::parse("# HELP missing_value\n").unwrap_err();
    assert!(err.to_string().contains("HELP"));

    let err = OpenMetricsDocument::parse("# TYPE requests magic\n").unwrap_err();
    assert!(err.to_string().contains("unknown metric type"));

    let err = OpenMetricsDocument::parse("# UNIT requests\n").unwrap_err();
    assert!(err.to_string().contains("UNIT"));
}

#[test]
fn parser_rejects_malformed_exemplars_and_escapes() {
    let err = OpenMetricsDocument::parse("x 1 # {trace_id=\"abc\"}\n").unwrap_err();
    assert!(err.to_string().contains("exemplar"));

    let err = OpenMetricsDocument::parse("x 1 # {trace_id=\"abc\"} 1 2 3\n").unwrap_err();
    assert!(err.to_string().contains("too many exemplar"));

    let err = OpenMetricsDocument::parse("x{label=\"bad\\t\"} 1\n").unwrap_err();
    assert!(err.to_string().contains("invalid label escape"));

    let err = OpenMetricsDocument::parse("x{label=\"unterminated} 1\n").unwrap_err();
    assert!(err.to_string().contains("unterminated label"));
}

#[test]
fn parser_infers_families_from_common_suffixes() {
    let doc = OpenMetricsDocument::parse(concat!(
        "requests_total 1\n",
        "latency_bucket{le=\"+Inf\"} 1\n",
        "latency_sum 0.1\n",
        "latency_count 1\n",
        "build_info{version=\"1\"} 1\n",
        "state{state=\"running\"} 1\n",
        "# EOF\n",
    ))
    .unwrap();

    for family in ["requests", "latency", "build", "state"] {
        assert!(doc.family(family).is_some(), "missing family {}", family);
    }
}
