//! Leaf metric entries behind one kind-parameterized builder.
//!
//! [`EntryBuilder`]`<K>` is the single start builder for every leaf kind; the
//! sealed markers in [`kind`] pick the finisher and the finished entry:
//! [`LensEntry`] for metrics borrowed through a [`Lens`] (scalar
//! counters/gauges/info and metric-tree nodes), [`ValueEntry`] for values
//! computed from the runtime context. The per-kind describe/collect behavior
//! lives in the [`kind::LensKind`] impls at the bottom of this file — the only
//! place the kinds actually differ.

use super::kind::{self, LabeledKind, LensKind, ValueKind};
use super::{EntryMetadata, Lens, MetricEntry};
use crate::labels::slices::with_labels;
use crate::schema::{MetricSchema, SchemaError};
use crate::values::MetricValues;
use crate::{CounterSource, GaugeSource, Info, MetricTree, MetricType, Name, Scalar};
use std::fmt;
use std::marker::PhantomData;

/// Start builder for a leaf metric entry of kind `K`.
///
/// Obtained from the free functions ([`counter`], [`gauge`], [`info`],
/// [`metric`], [`counter_value`], [`gauge_value`]) — call sites never need to
/// name this type. The kind decides which finisher applies:
/// [`source`](EntryBuilder::source) / [`select`](EntryBuilder::select) for
/// lens kinds, [`read`](EntryBuilder::read) for value kinds.
#[must_use = "an entry builder does nothing until the entry is registered"]
pub struct EntryBuilder<K> {
    metadata: EntryMetadata,
    _kind: PhantomData<K>,
}

impl<K> Clone for EntryBuilder<K> {
    fn clone(&self) -> Self {
        EntryBuilder {
            metadata: self.metadata.clone(),
            _kind: PhantomData,
        }
    }
}

impl<K> fmt::Debug for EntryBuilder<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EntryBuilder")
            .field("metadata", &self.metadata)
            .finish()
    }
}

/// A leaf entry of kind `K`: a metric borrowed through a lens, sourced
/// directly or selected from the runtime context at scrape time.
#[must_use = "a metric entry does nothing until registered"]
pub struct LensEntry<'a, K, C, M: ?Sized> {
    metadata: EntryMetadata,
    metric: Lens<'a, C, M>,
    _kind: PhantomData<K>,
}

/// A leaf entry of kind `K` whose value is computed from the runtime context
/// by the stored reader closure.
#[must_use = "a metric entry does nothing until registered"]
pub struct ValueEntry<K, F> {
    metadata: EntryMetadata,
    read: F,
    _kind: PhantomData<K>,
}

impl<K, F: Clone> Clone for ValueEntry<K, F> {
    fn clone(&self) -> Self {
        ValueEntry {
            metadata: self.metadata.clone(),
            read: self.read.clone(),
            _kind: PhantomData,
        }
    }
}

impl<K, F> fmt::Debug for ValueEntry<K, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValueEntry")
            .field("metadata", &self.metadata)
            .finish_non_exhaustive()
    }
}

impl<K: kind::EntryKind> EntryBuilder<K> {
    meta_setters!(help_unit);
}

impl<K: LabeledKind> EntryBuilder<K> {
    meta_setters!(label);
}

impl<K: kind::EntryKind, C, M: ?Sized> LensEntry<'_, K, C, M> {
    meta_setters!(help_unit);
}

impl<K: LabeledKind, C, M: ?Sized> LensEntry<'_, K, C, M> {
    meta_setters!(label);
}

impl<K: ValueKind, F> ValueEntry<K, F> {
    meta_setters!(help_unit);
}

impl<K> EntryBuilder<K> {
    fn start(name: impl Into<Name>) -> Self {
        EntryBuilder {
            metadata: EntryMetadata::new(name),
            _kind: PhantomData,
        }
    }

    /// Uses a direct metric source for a root [`crate::Registry`].
    pub fn source<M>(self, source: &M) -> LensEntry<'_, K, (), M>
    where
        M: ?Sized,
        K: LensKind<M>,
    {
        LensEntry {
            metadata: self.metadata,
            metric: source.into(),
            _kind: PhantomData,
        }
    }

    /// Selects a metric from a runtime view context.
    pub fn select<'f, C, M, F>(self, select: F) -> LensEntry<'f, K, C, M>
    where
        K: LensKind<M>,
        M: 'static,
        F: for<'ctx> Fn(&'ctx C) -> &'ctx M + Send + Sync + 'f,
    {
        LensEntry {
            metadata: self.metadata,
            metric: Lens::Projection(Box::new(select)),
            _kind: PhantomData,
        }
    }

    /// Reads a computed value from the runtime context.
    pub fn read<F>(self, read: F) -> ValueEntry<K, F>
    where
        K: ValueKind,
    {
        ValueEntry {
            metadata: self.metadata,
            read,
            _kind: PhantomData,
        }
    }
}

/// Starts a metric-tree entry builder.
pub fn metric(name: impl Into<Name>) -> EntryBuilder<kind::MetricTree> {
    EntryBuilder::start(name)
}

/// Starts a counter entry builder.
pub fn counter(name: impl Into<Name>) -> EntryBuilder<kind::Counter> {
    EntryBuilder::start(name)
}

/// Starts a gauge entry builder.
pub fn gauge(name: impl Into<Name>) -> EntryBuilder<kind::Gauge> {
    EntryBuilder::start(name)
}

/// Starts an info entry builder.
///
/// A **held** info metric (`.source(...)`, or `.select(...)` described with a
/// live context) enumerates its intrinsic labels itself. In a **context-free**
/// schema walk -- a `select`ed info inside a
/// [`family_view`](crate::MetricTreeView::family_view) element view -- the
/// target is unreachable, so declare the value's intrinsic label names on the
/// builder via [`label`](EntryBuilder::label); an undeclared projected info is
/// recorded as a [`SchemaError::UndescribedEntry`] (surfaced by
/// [`MetricSchema::validate`](crate::MetricSchema::validate)) rather than
/// declaring a schema its values would outgrow.
pub fn info(name: impl Into<Name>) -> EntryBuilder<kind::Info> {
    EntryBuilder::start(name)
}

/// Starts a computed counter value entry builder.
pub fn counter_value(name: impl Into<Name>) -> EntryBuilder<kind::CounterValue> {
    EntryBuilder::start(name)
}

/// Starts a computed gauge value entry builder.
pub fn gauge_value(name: impl Into<Name>) -> EntryBuilder<kind::GaugeValue> {
    EntryBuilder::start(name)
}

impl<K, C, M> MetricEntry<C> for LensEntry<'_, K, C, M>
where
    K: LensKind<M>,
    M: ?Sized,
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
        K::describe_target(
            self.metric.resolve(context),
            &self.metadata,
            name,
            labels,
            schema,
        );
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        match &self.metric {
            // Only a directly-held target can describe itself without a
            // context.
            Lens::Ref(metric) => K::describe_target(metric, &self.metadata, name, labels, schema),
            // A projected target is unreachable until an instance is supplied;
            // the kind decides whether its shape is still known statically
            // (scalars) or must be recorded as a schema gap (metric trees).
            Lens::Projection(_) => K::describe_untargeted(&self.metadata, name, labels, schema),
        }
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        K::collect(self.metric.resolve(context), name, labels, values);
    }

    fn housekeep(&self, context: &C) {
        if K::MAINTAINABLE {
            K::housekeep(self.metric.resolve(context));
        }
    }

    fn needs_housekeep(&self, context: &C) -> bool {
        K::MAINTAINABLE && K::needs_housekeep(self.metric.resolve(context))
    }
}

impl<C, F> MetricEntry<C> for ValueEntry<kind::CounterValue, F>
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
        declare_family(&self.metadata, MetricType::Counter, name, labels, schema);
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, (self.read)(context));
    }

    fn housekeep(&self, _: &C) {}
}

impl<C, F, V> MetricEntry<C> for ValueEntry<kind::GaugeValue, F>
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
        declare_family(&self.metadata, MetricType::Gauge, name, labels, schema);
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let value: Scalar = (self.read)(context).into();
        values.gauge(name, labels, value);
    }

    fn housekeep(&self, _: &C) {}
}

impl<M> LensKind<M> for kind::MetricTree
where
    M: MetricTree + ?Sized,
{
    fn describe_target(
        metric: &M,
        _metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        metric.describe(name, labels, schema);
    }

    fn describe_untargeted(
        _metadata: &EntryMetadata,
        name: &str,
        _labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        // A projected tree's shape is unreachable until an instance is
        // supplied: record the gap so `MetricSchema::validate` surfaces it
        // instead of the values silently outgrowing the schema.
        schema.record_error(SchemaError::UndescribedEntry {
            name: name.to_owned(),
        });
    }

    fn collect(metric: &M, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        metric.collect(name, labels, values);
    }

    const MAINTAINABLE: bool = true;

    fn housekeep(metric: &M) {
        if metric.needs_housekeep() {
            metric.housekeep();
        }
    }

    fn needs_housekeep(metric: &M) -> bool {
        metric.needs_housekeep()
    }
}

impl<M> LensKind<M> for kind::Counter
where
    M: CounterSource + ?Sized,
{
    fn describe_target(
        _metric: &M,
        metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        declare_family(metadata, MetricType::Counter, name, labels, schema);
    }

    fn describe_untargeted(
        metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        declare_family(metadata, MetricType::Counter, name, labels, schema);
    }

    fn collect(metric: &M, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, metric.get());
    }
}

impl<M> LensKind<M> for kind::Gauge
where
    M: GaugeSource + ?Sized,
{
    fn describe_target(
        _metric: &M,
        metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        declare_family(metadata, MetricType::Gauge, name, labels, schema);
    }

    fn describe_untargeted(
        metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        declare_family(metadata, MetricType::Gauge, name, labels, schema);
    }

    fn collect(metric: &M, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let value: Scalar = metric.get().into();
        values.gauge(name, labels, value);
    }
}

impl<M> LensKind<M> for kind::Info
where
    M: Info + ?Sized,
{
    fn describe_target(
        metric: &M,
        _metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        // A held info metric can enumerate its intrinsic labels.
        let info_labels = metric.labels();
        let all = with_labels(
            labels,
            info_labels
                .as_slice()
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
        schema.add_family(name, MetricType::Info, &all);
    }

    fn describe_untargeted(
        metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        // A projected info metric cannot enumerate its intrinsic labels
        // without a target, but `collect` will emit them -- so declaring the
        // family from the metadata alone would advertise a shape the values
        // outgrow. The entry's declared `.label(...)` names stand in for the
        // intrinsics when the caller provided them (the same caller-declared
        // trust as `Family::with_label_names`); without a declaration the
        // shape is unknowable here, so record the gap as an
        // `UndescribedEntry` for `MetricSchema::validate` to surface, exactly
        // like a projected metric tree.
        if metadata.labels.is_empty() {
            schema.record_error(SchemaError::UndescribedEntry {
                name: name.to_owned(),
            });
        } else {
            declare_family(metadata, MetricType::Info, name, labels, schema);
        }
    }

    fn collect(metric: &M, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let info_labels = metric.labels();
        let all = with_labels(
            labels,
            info_labels
                .as_slice()
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
        values.sample(&format!("{name}_info"), &all, 1u64);
    }
}

/// Declares one family from the entry's own metadata: the shared context-free
/// schema shape of scalar and computed-value entries.
fn declare_family(
    metadata: &EntryMetadata,
    metric_type: MetricType,
    name: &str,
    labels: &[(&str, &str)],
    schema: &mut MetricSchema,
) {
    schema.add_family_with_const_labels(
        name,
        metric_type,
        labels,
        &metadata.declared_label_names(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MetricTreeView;
    use crate::{InfoMetric, SchemaError};
    use std::sync::atomic::AtomicU64;

    struct Member {
        requests: AtomicU64,
        build: InfoMetric,
    }

    fn members() -> Vec<(String, Member)> {
        vec![(
            "a".to_owned(),
            Member {
                requests: AtomicU64::new(0),
                build: InfoMetric::new([("version", "1.0")]),
            },
        )]
    }

    fn group_view<'a>(
        element: MetricTreeView<'a, Member>,
    ) -> MetricTreeView<'a, Vec<(String, Member)>> {
        let mut view = MetricTreeView::new();
        view.family_by("member", element, |members: &Vec<(String, Member)>, out| {
            for (key, member) in members {
                out.emit(key, member);
            }
        });
        view
    }

    #[test]
    fn projected_info_with_declared_labels_matches_its_values_in_a_family_view() {
        // A `select`ed info inside a `family_view`/`family_by` group is
        // described context-free: its declared `.label(...)` names must make
        // the schema carry exactly the label names the collected values carry.
        let mut element = MetricTreeView::<Member>::new();
        element.register(
            info("build")
                .select(|member: &Member| &member.build)
                .label("version"),
        );
        let view = group_view(element);
        let members = members();

        let schema = view.schema(&members);
        assert!(schema.validate().is_ok(), "{:?}", schema.validate());
        let family = schema.family("build").expect("family described");
        let mut schema_labels = family.labels.clone();
        schema_labels.sort();

        let values = view.values(&members);
        let sample = values
            .samples()
            .iter()
            .find(|sample| sample.name == "build_info")
            .expect("info sample collected");
        let mut value_labels: Vec<String> =
            sample.labels.iter().map(|(name, _)| name.clone()).collect();
        value_labels.sort();

        assert_eq!(
            schema_labels, value_labels,
            "schema and values must agree on the info series' label names"
        );
        assert_eq!(schema_labels, vec!["member", "version"]);
    }

    #[test]
    fn projected_info_without_declared_labels_is_recorded_as_undescribed() {
        // Without declared intrinsic label names the context-free shape is
        // unknowable, so the entry must surface through the UndescribedEntry
        // mechanism instead of declaring a schema its values outgrow.
        let mut element = MetricTreeView::<Member>::new();
        // A describable sibling keeps the element view's context-free schema
        // non-empty (an all-undescribable element view is a debug_assert at
        // registration).
        element.register(counter("requests").select(|member: &Member| &member.requests));
        element.register(info("build").select(|member: &Member| &member.build));
        let view = group_view(element);

        let schema = view.schema(&members());
        assert!(
            schema.family("build").is_none(),
            "an undescribable family must not be declared with a lying shape"
        );
        assert_eq!(
            schema.validate().unwrap_err(),
            vec![SchemaError::UndescribedEntry {
                name: "build".to_owned(),
            }]
        );
    }
}
