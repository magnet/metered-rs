//! Hyper 1.x response helpers for OpenMetrics text.
//!
//! This module intentionally does not bind sockets, spawn tasks, implement TLS,
//! or make request-routing decisions. Framework code owns those concerns and
//! can call these response builders from its own Hyper 1 service.

use crate::{HistogramProfile, OpenMetricsEncoder, RegistrySnapshot, Snapshot, SnapshotTarget};
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Response, StatusCode};
use metered::{MetricSchema, MetricTree, MetricValues, Registry};
use std::fmt;

/// Hyper 1.x response body type used by these helpers.
pub type Body = Full<Bytes>;

/// OpenMetrics text content type.
pub const CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// Builds a response from an already rendered snapshot.
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
) -> Result<Response<Body>, fmt::Error> {
    let mut text = String::new();
    {
        let mut encoder = OpenMetricsEncoder::new(&mut text).histogram_profile(profile);
        encoder.encode_document(schema, values)?;
        encoder.finish()?;
    }
    Ok(response_from_bytes(Bytes::from(text)))
}

/// Renders a [`Registry`] into an OpenMetrics HTTP response.
pub fn response_from_registry(
    registry: &Registry<'_>,
    profile: HistogramProfile,
) -> Result<Response<Body>, fmt::Error> {
    let target = RegistrySnapshot::new(registry);
    response_from_document(&target.schema(), &target.values(), profile)
}

/// Renders a [`MetricTree`] into an OpenMetrics HTTP response.
pub fn response_from_tree<T: MetricTree>(
    tree: &T,
    profile: HistogramProfile,
) -> Result<Response<Body>, fmt::Error> {
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
    use metered::Registry;
    use std::sync::atomic::AtomicU64;

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
        assert!(std::str::from_utf8(body.chunk())
            .unwrap()
            .contains("requests_total 3"));
    }
}
