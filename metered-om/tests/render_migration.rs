//! Migration dual-shape rendering through the OpenMetrics sink. Gated on the
//! `migration` feature, which enables `metered`'s migration module.
#![cfg(feature = "migration")]

use metered::bucket_histogram::Buckets;
use metered::{BucketHistogram, Registry};
use metered_om::OpenMetricsRegistryExt;
use metered_semantic::migration::{LegacySummary, WithLegacySummary};

fn histogram_with(observations: &[f64]) -> BucketHistogram {
    let histogram = BucketHistogram::new(Buckets::custom([1.0, 2.0, 3.0, 4.0]));
    for &value in observations {
        histogram.observe(value);
    }
    histogram
}

#[test]
fn dual_registration_exposes_both_shapes_from_one_histogram() {
    let latency = histogram_with(&[0.5, 1.5, 2.5]);
    let legacy = LegacySummary::new(&latency).with_quantiles([0.5, 0.99]);
    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("http_request_duration_seconds")
            .source(&latency)
            .help("Latency"),
    );
    registry.register(
        metered::entry::metric("response_time")
            .source(&legacy)
            .help("Legacy summary"),
    );

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# TYPE http_request_duration_seconds histogram"));
    assert!(text.contains("http_request_duration_seconds_bucket{le=\"1\"} 1"));
    assert!(text.contains("# TYPE response_time summary"));
    assert!(text.contains("response_time{quantile=\"0.5\"}"));
    assert!(text.contains("response_time{quantile=\"0.99\"}"));
    assert!(text.contains("response_time_count 3"));
}

#[test]
fn wrapper_emits_both_shapes_from_one_registration() {
    let latency = WithLegacySummary::new(histogram_with(&[0.5, 1.5, 2.5]), "response_time")
        .with_quantiles([0.5, 0.99])
        .legacy_help("Legacy latency summary");

    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric("http_request_duration_seconds")
            .source(&latency)
            .help("Request latency"),
    );

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# TYPE http_request_duration_seconds histogram"));
    assert!(text.contains("http_request_duration_seconds_count 3"));
    assert!(text.contains("# HELP response_time Legacy latency summary"));
    assert!(text.contains("# TYPE response_time summary"));
    assert!(text.contains("response_time{quantile=\"0.5\"}"));
    assert!(text.contains("response_time{quantile=\"0.99\"}"));
    assert!(text.contains("response_time_count 3"));
}
