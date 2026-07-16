//! The exposition sink: where a described-and-collected metric tree is written.
//!
//! Core describes a [`MetricSchema`] and collects a
//! [`MetricValues`]; a *sink* turns that pair into a
//! concrete exposition format. This trait is the seam between the
//! instrumentation core and the (separately crated) exporters, so a service
//! picks whichever sink it needs -- the OpenMetrics text encoder in
//! `metered-om`, a future Prometheus-protobuf encoder, an in-memory
//! collector for tests -- without the core depending on any of them.

use crate::schema::MetricSchema;
use crate::values::MetricValues;
use std::fmt;

/// A destination that can encode a metric document from its schema and values.
///
/// The `encode(.., &mut dyn MetricSink)` methods on
/// [`MetricTree`](crate::MetricTree), [`Registry`](crate::Registry), and
/// [`MetricTreeView`](crate::MetricTreeView) are generic over this trait, so the
/// same metric tree can be rendered by any sink.
pub trait MetricSink {
    /// Encodes one document from a `schema` and its sampled `values`.
    fn encode_document(&mut self, schema: &MetricSchema, values: &MetricValues) -> fmt::Result;
}
