//! Opaque entry: an explicitly-typed family whose values are produced by a
//! caller-supplied collect closure rather than borrowed from a metric.

use super::{EntryMetadata, MetricEntry};
use crate::schema::MetricSchema;
use crate::values::MetricValues;
use crate::{Help, LabelName, MetricType, Name, Unit};

type OpaqueCollect<'a> = dyn Fn(&str, &[(&str, &str)], &mut MetricValues) + Send + Sync + 'a;

pub(crate) struct OpaqueEntry<'a> {
    metadata: EntryMetadata,
    metric_type: MetricType,
    collect: Box<OpaqueCollect<'a>>,
}

impl<'a> OpaqueEntry<'a> {
    pub(crate) fn new(
        name: impl Into<Name>,
        help: impl Into<Help>,
        unit: Option<Unit>,
        metric_type: MetricType,
        labels: impl IntoIterator<Item = impl Into<LabelName>>,
        collect: impl Fn(&str, &[(&str, &str)], &mut MetricValues) + Send + Sync + 'a,
    ) -> Self {
        OpaqueEntry {
            metadata: EntryMetadata {
                name: name.into(),
                help: Some(help.into()),
                unit,
                labels: labels.into_iter().map(Into::into).collect(),
            },
            metric_type,
            collect: Box::new(collect),
        }
    }
}

impl MetricEntry<()> for OpaqueEntry<'_> {
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(&self, _: &(), name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.describe_schema(name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        schema.add_family_with_const_labels(
            name,
            self.metric_type,
            labels,
            &self.metadata.declared_label_names(),
        );
    }

    fn collect(&self, _: &(), name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        (self.collect)(name, labels, values);
    }

    fn housekeep(&self, _: &()) {}
}
