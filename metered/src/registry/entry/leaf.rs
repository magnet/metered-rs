//! Leaf metric entries: scalar counters/gauges/info, the metric-tree node, and
//! the computed-value readers. Scalar and metric-tree entries resolve their
//! metric through a [`Lens`].

use super::{EntryMetadata, Lens, MetricEntry};
use crate::labels::slices::with_labels;
use crate::schema::{MetricSchema, SchemaError};
use crate::values::MetricValues;
use crate::{CounterSource, GaugeSource, Info, MetricTree, MetricType, Name, Scalar};

/// Start builder for a metric-tree entry.
#[derive(Clone, Debug)]
pub struct MetricBuilder {
    metadata: EntryMetadata,
}

/// Start builder for a counter entry.
#[derive(Clone, Debug)]
pub struct CounterBuilder {
    metadata: EntryMetadata,
}

/// Start builder for a gauge entry.
#[derive(Clone, Debug)]
pub struct GaugeBuilder {
    metadata: EntryMetadata,
}

/// Start builder for an info entry.
#[derive(Clone, Debug)]
pub struct InfoBuilder {
    metadata: EntryMetadata,
}

/// Start builder for a computed counter value entry.
#[derive(Clone, Debug)]
pub struct CounterValueBuilder {
    metadata: EntryMetadata,
}

/// Start builder for a computed gauge value entry.
#[derive(Clone, Debug)]
pub struct GaugeValueBuilder {
    metadata: EntryMetadata,
}

/// A metric-tree entry, sourced directly or selected from a runtime context.
pub struct MetricTreeEntry<'a, C, M: ?Sized> {
    metadata: EntryMetadata,
    metric: Lens<'a, C, M>,
}

/// A counter entry, sourced directly or selected from a runtime context.
pub struct CounterEntry<'a, C, M: ?Sized> {
    metadata: EntryMetadata,
    metric: Lens<'a, C, M>,
}

/// A gauge entry, sourced directly or selected from a runtime context.
pub struct GaugeEntry<'a, C, M: ?Sized> {
    metadata: EntryMetadata,
    metric: Lens<'a, C, M>,
}

/// An info entry, sourced directly or selected from a runtime context.
pub struct InfoEntry<'a, C, M: ?Sized> {
    metadata: EntryMetadata,
    metric: Lens<'a, C, M>,
}

/// A computed counter value entry.
#[derive(Clone, Debug)]
pub struct CounterValueEntry<F> {
    metadata: EntryMetadata,
    read: F,
}

/// A computed gauge value entry.
#[derive(Clone, Debug)]
pub struct GaugeValueEntry<F> {
    metadata: EntryMetadata,
    read: F,
}

// A metric-tree entry's inner tree owns its label declarations, so the tree
// builder/entry deliberately have no `label` setter (it would be dropped).
impl_inherent_meta!(@meta_only MetricBuilder);
impl_inherent_meta!(CounterBuilder);
impl_inherent_meta!(GaugeBuilder);
impl_inherent_meta!(InfoBuilder);
impl_inherent_meta!(@meta_only_unsized MetricTreeEntry<'a, C, M>);
impl_inherent_meta!(@unsized CounterEntry<'a, C, M>);
impl_inherent_meta!(@unsized GaugeEntry<'a, C, M>);
impl_inherent_meta!(@unsized InfoEntry<'a, C, M>);

impl<F> CounterValueEntry<F> {
    /// Attaches OpenMetrics `# HELP` text to this entry.
    pub fn help(mut self, help: impl Into<crate::Help>) -> Self {
        self.metadata.help = Some(help.into());
        self
    }

    /// Attaches an OpenMetrics `# UNIT` value to this entry.
    pub fn unit(mut self, unit: impl Into<crate::Unit>) -> Self {
        self.metadata.unit = Some(unit.into());
        self
    }
}

impl<F> GaugeValueEntry<F> {
    /// Attaches OpenMetrics `# HELP` text to this entry.
    pub fn help(mut self, help: impl Into<crate::Help>) -> Self {
        self.metadata.help = Some(help.into());
        self
    }

    /// Attaches an OpenMetrics `# UNIT` value to this entry.
    pub fn unit(mut self, unit: impl Into<crate::Unit>) -> Self {
        self.metadata.unit = Some(unit.into());
        self
    }
}

impl MetricBuilder {
    /// Uses a direct metric-tree source for a root [`crate::Registry`].
    pub fn source<M>(self, source: &M) -> MetricTreeEntry<'_, (), M>
    where
        M: MetricTree + ?Sized,
    {
        MetricTreeEntry {
            metadata: self.metadata,
            metric: source.into(),
        }
    }

    /// Selects a metric tree from a runtime view context.
    pub fn select<'f, C, M, F>(self, select: F) -> MetricTreeEntry<'f, C, M>
    where
        F: for<'ctx> Fn(&'ctx C) -> &'ctx M + 'f,
        M: MetricTree + 'static,
    {
        MetricTreeEntry {
            metadata: self.metadata,
            metric: Lens::Projection(Box::new(select)),
        }
    }
}

impl CounterBuilder {
    /// Uses a direct counter source for a root [`crate::Registry`].
    pub fn source<T>(self, source: &T) -> CounterEntry<'_, (), T>
    where
        T: CounterSource + ?Sized,
    {
        CounterEntry {
            metadata: self.metadata,
            metric: source.into(),
        }
    }

    /// Selects a counter from a runtime view context.
    pub fn select<'f, C, T, F>(self, select: F) -> CounterEntry<'f, C, T>
    where
        F: for<'ctx> Fn(&'ctx C) -> &'ctx T + 'f,
        T: CounterSource + 'static,
    {
        CounterEntry {
            metadata: self.metadata,
            metric: Lens::Projection(Box::new(select)),
        }
    }
}

impl GaugeBuilder {
    /// Uses a direct gauge source for a root [`crate::Registry`].
    pub fn source<T>(self, source: &T) -> GaugeEntry<'_, (), T>
    where
        T: GaugeSource + ?Sized,
    {
        GaugeEntry {
            metadata: self.metadata,
            metric: source.into(),
        }
    }

    /// Selects a gauge from a runtime view context.
    pub fn select<'f, C, T, F>(self, select: F) -> GaugeEntry<'f, C, T>
    where
        F: for<'ctx> Fn(&'ctx C) -> &'ctx T + 'f,
        T: GaugeSource + 'static,
    {
        GaugeEntry {
            metadata: self.metadata,
            metric: Lens::Projection(Box::new(select)),
        }
    }
}

impl InfoBuilder {
    /// Uses a direct info source for a root [`crate::Registry`].
    pub fn source<T>(self, source: &T) -> InfoEntry<'_, (), T>
    where
        T: Info + ?Sized,
    {
        InfoEntry {
            metadata: self.metadata,
            metric: source.into(),
        }
    }

    /// Selects an info metric from a runtime view context.
    pub fn select<'f, C, T, F>(self, select: F) -> InfoEntry<'f, C, T>
    where
        F: for<'ctx> Fn(&'ctx C) -> &'ctx T + 'f,
        T: Info + 'static,
    {
        InfoEntry {
            metadata: self.metadata,
            metric: Lens::Projection(Box::new(select)),
        }
    }
}

impl CounterValueBuilder {
    /// Reads a computed counter value from the runtime context.
    pub fn read<F>(self, read: F) -> CounterValueEntry<F> {
        CounterValueEntry {
            metadata: self.metadata,
            read,
        }
    }
}

impl GaugeValueBuilder {
    /// Reads a computed gauge value from the runtime context.
    pub fn read<F>(self, read: F) -> GaugeValueEntry<F> {
        GaugeValueEntry {
            metadata: self.metadata,
            read,
        }
    }
}

/// Starts a metric-tree entry builder.
pub fn metric(name: impl Into<Name>) -> MetricBuilder {
    MetricBuilder {
        metadata: EntryMetadata::new(name),
    }
}

/// Starts a counter entry builder.
pub fn counter(name: impl Into<Name>) -> CounterBuilder {
    CounterBuilder {
        metadata: EntryMetadata::new(name),
    }
}

/// Starts a gauge entry builder.
pub fn gauge(name: impl Into<Name>) -> GaugeBuilder {
    GaugeBuilder {
        metadata: EntryMetadata::new(name),
    }
}

/// Starts an info entry builder.
pub fn info(name: impl Into<Name>) -> InfoBuilder {
    InfoBuilder {
        metadata: EntryMetadata::new(name),
    }
}

/// Starts a computed counter value entry builder.
pub fn counter_value(name: impl Into<Name>) -> CounterValueBuilder {
    CounterValueBuilder {
        metadata: EntryMetadata::new(name),
    }
}

/// Starts a computed gauge value entry builder.
pub fn gauge_value(name: impl Into<Name>) -> GaugeValueBuilder {
    GaugeValueBuilder {
        metadata: EntryMetadata::new(name),
    }
}

impl<'a, C, M> MetricEntry<C> for MetricTreeEntry<'a, C, M>
where
    M: MetricTree + ?Sized,
{
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(
        &self,
        context: &C,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        self.metric.resolve(context).describe(name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        match &self.metric {
            // Only a directly-held tree can describe itself without a context.
            Lens::Ref(tree) => tree.describe(name, labels, schema),
            // A projected tree's shape is unreachable until an instance is
            // supplied: record the gap so `MetricSchema::validate` surfaces it
            // instead of the values silently outgrowing the schema.
            Lens::Projection(_) => schema.record_error(SchemaError::UndescribedEntry {
                name: name.to_owned(),
            }),
        }
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.metric.resolve(context).collect(name, labels, values);
    }

    fn housekeep(&self, context: &C) {
        let metric = self.metric.resolve(context);
        if metric.needs_housekeep() {
            metric.housekeep();
        }
    }

    fn needs_housekeep(&self, context: &C) -> bool {
        self.metric.resolve(context).needs_housekeep()
    }
}

impl<'a, C, M> MetricEntry<C> for CounterEntry<'a, C, M>
where
    M: CounterSource + ?Sized,
{
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(&self, _: &C, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.describe_schema(name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        schema.add_family_with_const_labels(
            name,
            MetricType::Counter,
            labels,
            &self.metadata.declared_label_names(),
        );
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, self.metric.resolve(context).get());
    }

    fn housekeep(&self, _: &C) {}
}

impl<'a, C, M> MetricEntry<C> for GaugeEntry<'a, C, M>
where
    M: GaugeSource + ?Sized,
{
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(&self, _: &C, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.describe_schema(name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        schema.add_family_with_const_labels(
            name,
            MetricType::Gauge,
            labels,
            &self.metadata.declared_label_names(),
        );
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let value: Scalar = self.metric.resolve(context).get().into();
        values.gauge(name, labels, value);
    }

    fn housekeep(&self, _: &C) {}
}

impl<'a, C, M> MetricEntry<C> for InfoEntry<'a, C, M>
where
    M: Info + ?Sized,
{
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(
        &self,
        context: &C,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        describe_info(self.metric.resolve(context), name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        match &self.metric {
            // A held info metric can enumerate its intrinsic labels.
            Lens::Ref(info) => describe_info(*info, name, labels, schema),
            // A projected one cannot; declare the family with what is known.
            Lens::Projection(_) => schema.add_family_with_const_labels(
                name,
                MetricType::Info,
                labels,
                &self.metadata.declared_label_names(),
            ),
        }
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        collect_info(self.metric.resolve(context), name, labels, values);
    }

    fn housekeep(&self, _: &C) {}
}

impl<C, F> MetricEntry<C> for CounterValueEntry<F>
where
    F: Fn(&C) -> u64,
{
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(&self, _: &C, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.describe_schema(name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        schema.add_family_with_const_labels(
            name,
            MetricType::Counter,
            labels,
            &self.metadata.declared_label_names(),
        );
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, (self.read)(context));
    }

    fn housekeep(&self, _: &C) {}
}

impl<C, F, V> MetricEntry<C> for GaugeValueEntry<F>
where
    F: Fn(&C) -> V,
    V: Into<Scalar>,
{
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(&self, _: &C, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.describe_schema(name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        schema.add_family_with_const_labels(
            name,
            MetricType::Gauge,
            labels,
            &self.metadata.declared_label_names(),
        );
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let value: Scalar = (self.read)(context).into();
        values.gauge(name, labels, value);
    }

    fn housekeep(&self, _: &C) {}
}

fn describe_info(
    info: &(impl Info + ?Sized),
    name: &str,
    labels: &[(&str, &str)],
    schema: &mut MetricSchema,
) {
    let info_labels = info.labels();
    let all = with_labels(
        labels,
        info_labels
            .as_slice()
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    schema.add_family(name, MetricType::Info, &all);
}

fn collect_info(
    info: &(impl Info + ?Sized),
    name: &str,
    labels: &[(&str, &str)],
    values: &mut MetricValues,
) {
    let info_labels = info.labels();
    let all = with_labels(
        labels,
        info_labels
            .as_slice()
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str())),
    );
    values.sample(&format!("{name}_info"), &all, 1u64);
}
