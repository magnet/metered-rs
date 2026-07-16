//! Type-level kinds for leaf metric entries.
//!
//! A kind marker selects the shape and behavior of an
//! [`EntryBuilder`](super::EntryBuilder): which finisher applies
//! ([`source`](super::EntryBuilder::source) /
//! [`select`](super::EntryBuilder::select) for lens kinds,
//! [`read`](super::EntryBuilder::read) for value kinds), whether the entry
//! accepts dynamic [`label`](super::EntryBuilder::label) declarations, and how
//! the finished entry describes and collects its target.
//!
//! The traits here are **sealed**: the set of kinds is fixed by this crate.
//! Users never name kinds directly — the [`entry`](crate::registry::entry)
//! free functions pick them — and custom behavior plugs in at the
//! [`MetricEntry`](super::MetricEntry) trait instead.

use super::EntryMetadata;
use crate::schema::MetricSchema;
use crate::values::MetricValues;

mod sealed {
    pub trait Sealed {}
}

/// A leaf entry kind. Sealed; implemented only by the markers in this module.
pub trait EntryKind: sealed::Sealed {}

/// A kind whose schema honors dynamic label declarations, enabling the
/// [`label`](super::EntryBuilder::label) setter. Metric-tree entries own their
/// label declarations internally and deliberately do not implement this.
pub trait LabeledKind: EntryKind {}

/// A kind whose entry borrows a target metric `M` through a lens and knows how
/// to describe and collect it. Sealed; the blanket impls in this crate tie
/// each kind to its target bound (`CounterSource`, `GaugeSource`, `Info`,
/// `MetricTree`).
pub trait LensKind<M: ?Sized>: EntryKind {
    #[doc(hidden)]
    fn describe_target(
        metric: &M,
        metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    );

    /// Describes the schema when no target is reachable (a projected target
    /// without a runtime context).
    #[doc(hidden)]
    fn describe_untargeted(
        metadata: &EntryMetadata,
        name: &str,
        labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    );

    #[doc(hidden)]
    fn collect(metric: &M, name: &str, labels: &[(&str, &str)], values: &mut MetricValues);

    /// Whether this kind's target has off-hot-path maintenance. When `false`
    /// the entry never resolves its lens outside describe/collect.
    #[doc(hidden)]
    const MAINTAINABLE: bool = false;

    #[doc(hidden)]
    fn housekeep(metric: &M) {
        let _ = metric;
    }

    #[doc(hidden)]
    fn needs_housekeep(metric: &M) -> bool {
        let _ = metric;
        false
    }
}

/// A kind whose entry reads a computed value from the runtime context,
/// enabling the [`read`](super::EntryBuilder::read) finisher.
pub trait ValueKind: EntryKind {}

/// Kind of a [`metric`](super::metric) entry: a whole self-describing
/// [`MetricTree`](crate::MetricTree) node.
pub enum MetricTree {}

/// Kind of a [`counter`](super::counter) entry: a
/// [`CounterSource`](crate::CounterSource) scalar.
pub enum Counter {}

/// Kind of a [`gauge`](super::gauge) entry: a
/// [`GaugeSource`](crate::GaugeSource) scalar.
pub enum Gauge {}

/// Kind of an [`info`](super::info) entry: an [`Info`](crate::Info) metric.
pub enum Info {}

/// Kind of a [`counter_value`](super::counter_value) entry: a counter computed
/// from the runtime context.
pub enum CounterValue {}

/// Kind of a [`gauge_value`](super::gauge_value) entry: a gauge computed from
/// the runtime context.
pub enum GaugeValue {}

impl sealed::Sealed for MetricTree {}
impl sealed::Sealed for Counter {}
impl sealed::Sealed for Gauge {}
impl sealed::Sealed for Info {}
impl sealed::Sealed for CounterValue {}
impl sealed::Sealed for GaugeValue {}

impl EntryKind for MetricTree {}
impl EntryKind for Counter {}
impl EntryKind for Gauge {}
impl EntryKind for Info {}
impl EntryKind for CounterValue {}
impl EntryKind for GaugeValue {}

impl LabeledKind for Counter {}
impl LabeledKind for Gauge {}
impl LabeledKind for Info {}

impl ValueKind for CounterValue {}
impl ValueKind for GaugeValue {}
