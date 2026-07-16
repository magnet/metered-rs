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
use std::error::Error;
use std::fmt;

/// The error a [`MetricSink`] reports when it cannot encode a document.
///
/// The sink seam owns its error type rather than borrowing one wire format's:
/// a text encoder's [`fmt::Error`] converts through `From` without allocating
/// (so the `?` operator keeps the text path zero-cost), while a sink with a
/// richer failure -- a protobuf encoder, an in-memory collector hitting a
/// capacity limit -- carries it as a boxed [`source`](Error::source) via
/// [`SinkError::from_source`].
///
/// [`Display`](fmt::Display) renders only this error's own message; inspect
/// the underlying failure through [`Error::source`], as error-chain reporters
/// do.
#[derive(Debug)]
#[non_exhaustive]
pub struct SinkError {
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl SinkError {
    /// Wraps an underlying failure as the sink error's
    /// [`source`](Error::source).
    pub fn from_source(source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        SinkError {
            source: Some(source.into()),
        }
    }
}

/// The zero-cost conversion for text sinks: [`fmt::Error`] carries no
/// information, so no allocation happens and no source is recorded.
impl From<fmt::Error> for SinkError {
    fn from(_: fmt::Error) -> Self {
        SinkError { source: None }
    }
}

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("metric sink failed to encode the document")
    }
}

impl Error for SinkError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

/// A destination that can encode a metric document from its schema and values.
///
/// The `encode(.., &mut dyn MetricSink)` methods on
/// [`MetricTree`](crate::MetricTree), [`Registry`](crate::Registry), and
/// [`MetricTreeView`](crate::MetricTreeView) are generic over this trait, so the
/// same metric tree can be rendered by any sink.
pub trait MetricSink {
    /// Encodes one document from a `schema` and its sampled `values`.
    ///
    /// A text sink propagates its writer's [`fmt::Error`] through the
    /// allocation-free `From` conversion; any other sink wraps its own failure
    /// with [`SinkError::from_source`].
    fn encode_document(
        &mut self,
        schema: &MetricSchema,
        values: &MetricValues,
    ) -> Result<(), SinkError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Counter, MetricTree};
    use std::sync::atomic::AtomicU64;

    #[test]
    fn from_fmt_error_records_no_source() {
        let error = SinkError::from(fmt::Error);
        assert!(error.source().is_none());
        assert_eq!(
            error.to_string(),
            "metric sink failed to encode the document"
        );
    }

    #[test]
    fn from_source_exposes_the_underlying_failure() {
        let underlying = std::io::Error::other("buffer full");
        let error = SinkError::from_source(underlying);
        let source = error.source().expect("source must be recorded");
        assert_eq!(source.to_string(), "buffer full");
        // The top-level message stays the sink's own, per the source-chain
        // convention (no duplicated cause text).
        assert_eq!(
            error.to_string(),
            "metric sink failed to encode the document"
        );
    }

    /// A sink can fail with a non-`fmt` error and the failure travels through
    /// the `MetricTree::encode` seam intact.
    #[test]
    fn encode_propagates_a_sink_error_with_its_source() {
        struct FailingSink;
        impl MetricSink for FailingSink {
            fn encode_document(
                &mut self,
                _schema: &MetricSchema,
                _values: &MetricValues,
            ) -> Result<(), SinkError> {
                Err(SinkError::from_source(std::io::Error::other("boom")))
            }
        }

        let counter = AtomicU64::new(0);
        counter.incr();
        let error = counter
            .encode("requests", &[], &mut FailingSink)
            .expect_err("the sink's failure must propagate");
        assert_eq!(error.source().unwrap().to_string(), "boom");
    }

    /// The happy path through the new seam: a recording sink receives the
    /// described schema and collected values.
    #[test]
    fn encode_hands_schema_and_values_to_the_sink() {
        #[derive(Default)]
        struct RecordingSink {
            families: usize,
            samples: usize,
        }
        impl MetricSink for RecordingSink {
            fn encode_document(
                &mut self,
                schema: &MetricSchema,
                values: &MetricValues,
            ) -> Result<(), SinkError> {
                self.families += schema.families().len();
                self.samples += values.samples().len();
                Ok(())
            }
        }

        let counter = AtomicU64::new(0);
        counter.incr();
        let mut sink = RecordingSink::default();
        counter.encode("requests", &[], &mut sink).unwrap();
        assert_eq!(sink.families, 1);
        assert_eq!(sink.samples, 1);
    }
}
