//! Runtime metric views that compose metrics into one document.
//!
//! [`MetricTreeView`] is the context-aware implementation: it stores
//! self-describing entries, each owning its name, optional metadata, and the
//! source/collector needed to describe and collect values. [`Registry`] is a
//! borrowed root wrapper over a `MetricTreeView<()>`.

pub mod entry;
mod view;

pub use view::{MetricTreeView, MetricsView};

use crate::schema::MetricSchema;
use crate::sink::{MetricSink, SinkError};
use crate::values::MetricValues;
use crate::{Help, LabelName, MetricType, Name, Unit};
use std::fmt;

/// A borrowed root registry over metrics the application already owns.
#[derive(Default)]
pub struct Registry<'a>(MetricTreeView<'a, ()>);

impl<'a> Registry<'a> {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Registry::default()
    }

    /// Creates a registry whose metric names are prefixed with `prefix`.
    #[must_use]
    pub fn with_prefix(prefix: impl Into<Name>) -> Self {
        Registry(MetricTreeView::with_prefix(prefix))
    }

    /// Adds a constant label applied to every series in this registry.
    pub fn label(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.0.label(name, value);
        self
    }

    /// Registers one self-describing entry.
    pub fn register<E>(&mut self, entry: E) -> &mut Self
    where
        E: entry::MetricEntry<()> + Send + Sync + 'a,
    {
        self.0.register(entry);
        self
    }

    /// Registers a value with an explicit schema and a value collector.
    pub fn register_opaque(
        &mut self,
        name: impl Into<Name>,
        help: impl Into<Help>,
        metric_type: MetricType,
        labels: impl IntoIterator<Item = impl Into<LabelName>>,
        collect: impl Fn(&str, &[(&str, &str)], &mut MetricValues) + Send + Sync + 'a,
    ) -> &mut Self {
        self.register_opaque_inner(name, help, None, metric_type, labels, collect)
    }

    /// Registers a value with explicit schema, unit, and a value collector.
    pub fn register_opaque_with_unit(
        &mut self,
        name: impl Into<Name>,
        help: impl Into<Help>,
        unit: impl Into<Unit>,
        metric_type: MetricType,
        labels: impl IntoIterator<Item = impl Into<LabelName>>,
        collect: impl Fn(&str, &[(&str, &str)], &mut MetricValues) + Send + Sync + 'a,
    ) -> &mut Self {
        self.register_opaque_inner(name, help, Some(unit.into()), metric_type, labels, collect)
    }

    fn register_opaque_inner(
        &mut self,
        name: impl Into<Name>,
        help: impl Into<Help>,
        unit: Option<Unit>,
        metric_type: MetricType,
        labels: impl IntoIterator<Item = impl Into<LabelName>>,
        collect: impl Fn(&str, &[(&str, &str)], &mut MetricValues) + Send + Sync + 'a,
    ) -> &mut Self {
        self.0.register(entry::OpaqueEntry::new(
            name,
            help,
            unit,
            metric_type,
            labels,
            collect,
        ));
        self
    }

    /// Sets whether a scrape first runs [`housekeep`](Registry::housekeep).
    pub fn housekeep_on_scrape(&mut self, on: bool) -> &mut Self {
        self.0.housekeep_on_scrape(on);
        self
    }

    /// Runs off-hot-path maintenance over every registered metric.
    pub fn housekeep(&self) {
        self.0.housekeep(&());
    }

    /// Encodes all registered metrics into `sink`.
    pub fn encode(&self, sink: &mut dyn MetricSink) -> Result<(), SinkError> {
        self.0.encode(&(), sink)
    }

    /// Describes the OpenMetrics families this registry can emit.
    #[must_use]
    pub fn schema(&self) -> MetricSchema {
        self.0.schema(&())
    }

    /// Samples the current values this registry can emit.
    pub fn values(&self) -> MetricValues {
        self.0.values(&())
    }

    /// Samples current values without first running scrape-time maintenance.
    /// Use this when a surrounding cache or coordinator has already called
    /// [`housekeep`](Registry::housekeep) for the same scrape.
    pub fn values_without_housekeep(&self) -> MetricValues {
        self.0.values_without_housekeep(&())
    }
}

impl fmt::Debug for Registry<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Registry").field(&self.0).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::metric;
    use std::sync::atomic::{AtomicI64, AtomicU64};

    #[test]
    fn registry_and_view_are_debuggable() {
        let requests = AtomicU64::new(0);
        let mut registry = Registry::with_prefix("app");
        registry.label("env", "test");
        registry.register(metric("requests").source(&requests));

        let debug = format!("{registry:?}");
        assert!(debug.contains("Registry"), "{debug}");
        assert!(debug.contains("app"), "prefix is shown: {debug}");
        assert!(debug.contains("env"), "labels are shown: {debug}");
        assert!(
            debug.contains("entries: 1"),
            "entry count is shown: {debug}"
        );
    }

    #[test]
    fn registry_values_match_registered_metric_trees() {
        let requests = AtomicU64::new(0);
        crate::Counter::incr_by(&requests, 2);
        let depth = AtomicI64::new(0);
        crate::Gauge::set(&depth, 5);

        let mut registry = Registry::with_prefix("app");
        registry.label("env", "test");
        registry.register(metric("requests").source(&requests).help("Requests"));
        registry.register(metric("depth").source(&depth).help("Depth"));

        let values = registry.values();
        assert!(values.samples().iter().any(|sample| {
            sample.name == "app_requests_total"
                && sample.value.to_string() == "2"
                && sample.labels == vec![("env".to_owned(), "test".to_owned())]
        }));
        assert!(values.samples().iter().any(|sample| {
            sample.name == "app_depth"
                && sample.value.to_string() == "5"
                && sample.labels == vec![("env".to_owned(), "test".to_owned())]
        }));
    }
}
