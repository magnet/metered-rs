//! Hyper 1.x response helpers for OpenMetrics text.
//!
//! This module intentionally does not bind sockets, spawn tasks, implement TLS,
//! or make request-routing decisions. Framework code owns those concerns and
//! can call these response builders from its own Hyper 1 service.

use crate::{HistogramProfile, OpenMetricsEncoder, RegistrySnapshot, Snapshot, SnapshotTarget};
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Response, StatusCode};
use metered::{MetricSchema, MetricTree, MetricValues, Registry, SinkError};

/// Hyper 1.x response body type used by these helpers.
pub type Body = Full<Bytes>;

/// OpenMetrics text content type.
pub const CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// Builds a response from an already rendered snapshot.
///
/// No staleness check is applied: the snapshot is served however old it is. A
/// caller that must bound staleness compares [`Snapshot::created_at`] against
/// its own budget first.
pub fn response_from_snapshot(snapshot: &Snapshot) -> Response<Body> {
    response_from_bytes(snapshot.body())
}

/// Builds a response from rendered OpenMetrics bytes.
pub fn response_from_bytes(bytes: Bytes) -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .header(hyper::header::CONTENT_TYPE, CONTENT_TYPE)
        .body(Full::new(bytes))
        .expect("valid OpenMetrics response")
}

/// Renders `schema` + `values` into an OpenMetrics HTTP response.
pub fn response_from_document(
    schema: &MetricSchema,
    values: &MetricValues,
    profile: HistogramProfile,
) -> Result<Response<Body>, SinkError> {
    let mut text = String::new();
    {
        let mut encoder = OpenMetricsEncoder::new(&mut text).histogram_profile(profile);
        encoder.encode_document(schema, values)?;
        encoder.finish()?;
    }
    Ok(response_from_bytes(Bytes::from(text)))
}

/// Renders a [`Registry`] into an OpenMetrics HTTP response.
///
/// This is a scrape boundary: sampling goes through [`Registry::values`],
/// which by default drives [`Registry::housekeep`] first (the registry's
/// housekeep-on-scrape contract).
pub fn response_from_registry(
    registry: &Registry<'_>,
    profile: HistogramProfile,
) -> Result<Response<Body>, SinkError> {
    let target = RegistrySnapshot::new(registry);
    response_from_document(&target.schema(), &target.values(), profile)
}

/// Renders a [`MetricTree`] into an OpenMetrics HTTP response.
///
/// This is a scrape boundary, so it drives off-hot-path upkeep first when any
/// metric asks for it -- the same gate [`MetricTree::encode`] applies. Without
/// it, a [`metered::DynamicExponentialHistogram`] scraped only through this
/// helper would never downscale and would freeze its sampled exemplar windows.
pub fn response_from_tree<T: MetricTree>(
    tree: &T,
    profile: HistogramProfile,
) -> Result<Response<Body>, SinkError> {
    if tree.needs_housekeep() {
        tree.housekeep();
    }
    let mut schema = MetricSchema::new();
    tree.describe("", &[], &mut schema);
    let mut values = MetricValues::new();
    tree.collect("", &[], &mut values);
    response_from_document(&schema, &values, profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Buf;
    use http_body_util::BodyExt;
    use metered::shape::Renamed;
    use metered::{DynamicExponentialHistogram, Registry};
    use std::sync::atomic::AtomicU64;

    /// A dynamic histogram whose bucket table has saturated: 16 observations
    /// at the midpoints of 16 consecutive schema-5 buckets, past the 12-bucket
    /// downscale threshold of a 16-slot table. Until `housekeep` runs, the
    /// pending downscale leaves `needs_housekeep()` true and the schema at its
    /// starting resolution of 5; one downscale (to schema 4) pair-merges them
    /// into 8 buckets, back under the threshold.
    fn saturated_histogram() -> DynamicExponentialHistogram {
        let histogram = DynamicExponentialHistogram::with_params(5, 16);
        for k in 0..16 {
            histogram.observe(2f64.powf((f64::from(k) + 0.5) / 32.0));
        }
        assert!(
            histogram.needs_housekeep(),
            "setup must leave a downscale pending"
        );
        assert_eq!(histogram.schema(), 5, "setup must not have downscaled yet");
        histogram
    }

    async fn body_text(response: Response<Body>) -> String {
        let body = response.into_body().collect().await.unwrap().to_bytes();
        std::str::from_utf8(body.chunk()).unwrap().to_owned()
    }

    #[tokio::test]
    async fn response_from_tree_housekeeps_a_saturated_histogram() {
        let histogram = saturated_histogram();

        let response = response_from_tree(
            &Renamed::new("latency_seconds", &histogram),
            HistogramProfile::Le,
        )
        .unwrap();

        // The scrape itself must have driven the downscale (the gate
        // `MetricTree::encode` applies): the schema dropped and no rescale is
        // pending any more.
        assert_eq!(
            histogram.schema(),
            4,
            "response_from_tree must housekeep before sampling"
        );
        assert!(!histogram.needs_rescale());

        // And the rendered document reflects the merged (post-downscale)
        // buckets: all 16 observations survive the fold.
        let body = body_text(response).await;
        assert!(
            body.contains("latency_seconds_count 16"),
            "merged buckets must keep every observation:\n{body}"
        );
    }

    #[tokio::test]
    async fn response_from_registry_housekeeps_via_the_registry_scrape_path() {
        let histogram = saturated_histogram();
        let mut registry = Registry::new();
        registry.register(metered::entry::metric("latency_seconds").source(&histogram));

        let response = response_from_registry(&registry, HistogramProfile::Le).unwrap();

        // `response_from_registry` samples through `Registry::values`, whose
        // default is housekeep-on-scrape -- pin that this path maintains too.
        assert_eq!(
            histogram.schema(),
            4,
            "the registry scrape path must housekeep"
        );
        assert!(!histogram.needs_rescale());
        let body = body_text(response).await;
        assert!(body.contains("latency_seconds_count 16"), "{body}");
    }

    #[tokio::test]
    async fn response_from_snapshot_sets_content_type_and_body() {
        let requests = AtomicU64::new(0);
        metered::Counter::incr_by(&requests, 3);
        let mut registry = Registry::new();
        registry.register(
            metered::entry::metric("requests")
                .source(&requests)
                .help("Requests"),
        );
        let snapshot = crate::SnapshotCache::new(crate::RegistrySnapshot::new(&registry))
            .refresh()
            .unwrap();

        let response = response_from_snapshot(&snapshot);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(hyper::header::CONTENT_TYPE).unwrap(),
            CONTENT_TYPE
        );
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert!(
            std::str::from_utf8(body.chunk())
                .unwrap()
                .contains("requests_total 3")
        );
    }
}
