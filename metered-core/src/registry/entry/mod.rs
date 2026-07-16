//! Self-describing metric entries for runtime registries and views.
//!
//! Each entry owns its name and metadata plus the source or selector needed to
//! describe and collect a metric. Scalar and metric-tree entries borrow their
//! metric through an internal borrow-on-demand handle (the read-only dual of
//! [`std::borrow::Cow`]): a metric the registry already holds is kept as a
//! direct reference, while one selected from a runtime context is focused out of
//! that context at scrape time.
//!
//! # Design: free functions over two structural families
//!
//! Every entry starts from a free function -- [`counter`], [`gauge`], [`info`],
//! [`metric`], [`counter_value`], [`gauge_value`], [`tree`], [`family_view`],
//! [`family_by`] -- and
//! those free functions **are** the public API: call sites chain
//! `source`/`select`/`read`/`view` and the metadata setters without ever
//! naming the types they thread through.
//!
//! Behind the free functions sit two structural families:
//!
//! * **Leaf entries** describe and collect one metric (or one computed value).
//!   They share a single start builder, [`EntryBuilder`]`<K>`, parameterized by
//!   a type-level *kind* marker from [`kind`]. The kind decides which finisher
//!   applies and what the finished entry is: lens kinds ([`kind::Counter`],
//!   [`kind::Gauge`], [`kind::Info`], [`kind::MetricTree`]) borrow a metric --
//!   [`source`](EntryBuilder::source) / [`select`](EntryBuilder::select) --
//!   and finish as a [`LensEntry`]; value kinds ([`kind::CounterValue`],
//!   [`kind::GaugeValue`]) store a reader closure -- [`read`](EntryBuilder::read)
//!   -- and finish as a [`ValueEntry`].
//! * **View mounts** compose a whole child [`MetricTreeView`] rather than one
//!   metric: [`tree`] mounts a static subtree ([`TreeBuilder`] →
//!   [`TreeViewBuilder`] → [`TreeEntry`]) and [`family_view`] mounts a dynamic
//!   per-member group ([`FamilyViewEntry`], keyed by a typed
//!   [`LabelSet`](crate::labels::family::LabelSet)) with [`family_by`] as its
//!   single-string-key sugar. These keep their own types because the
//!   pipeline genuinely differs: a mount needs a child context *and* a child
//!   view before it is an entry.
//!
//! The kind traits are **sealed** ([`kind::EntryKind`] and its refinements):
//! the set of kinds is part of this module's design, not an extension point.
//! Custom entries plug in at the [`MetricEntry`] trait instead.
//!
//! All stored selector and reader closures are required to be `Send + Sync`,
//! so a `MetricTreeView<'static, C>` is itself `Send + Sync` and can be cached
//! in a `static`.
//!
//! [`MetricTreeView`]: crate::MetricTreeView

use crate::schema::{MetricSchema, SchemaError};
use crate::values::MetricValues;
use crate::{Help, LabelName, Name, Unit};

/// Metadata attached to a registered metric entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryMetadata {
    /// The metric family name segment under the enclosing view prefix.
    pub name: Name,
    /// Optional OpenMetrics `# HELP` text.
    pub help: Option<Help>,
    /// Optional OpenMetrics `# UNIT` value.
    pub unit: Option<Unit>,
    /// Declared dynamic label names for reader-style entries.
    pub labels: Vec<LabelName>,
}

impl EntryMetadata {
    fn new(name: impl Into<Name>) -> Self {
        let name = name.into();
        // A name segment is joined into OpenMetrics family names, so it must
        // stay within the metric-name charset (empty means "no segment", the
        // flatten case). Debug-only: a typo should fail a test, not a scrape.
        debug_assert!(
            name.as_str()
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "metric name segment `{}` contains characters outside [a-zA-Z0-9_]",
            name.as_str(),
        );
        EntryMetadata {
            name,
            help: None,
            unit: None,
            labels: Vec::new(),
        }
    }

    fn declared_label_names(&self) -> Vec<String> {
        self.labels
            .iter()
            .map(|label| label.as_str().to_owned())
            .collect()
    }
}

/// A runtime entry that can describe, collect, and maintain itself.
pub trait MetricEntry<C> {
    /// Returns the entry metadata.
    fn metadata(&self) -> &EntryMetadata;

    /// Describes this entry under `name` with inherited constant `labels`.
    fn describe(&self, context: &C, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema);

    /// Describes this entry's schema **without** a runtime context.
    ///
    /// A dynamic group ([`family_view`](crate::MetricTreeView::family_view))
    /// advertises its member family shape from the element view alone, so a
    /// scrape's schema
    /// never depends on which members happen to be live. Entries whose shape is
    /// known statically (scalars, computed values, opaque declarations, nested
    /// views of such entries) describe it here. An entry that can only
    /// describe itself through a runtime projection records a
    /// [`SchemaError::UndescribedEntry`] instead -- surfaced by
    /// [`MetricSchema::validate`] -- so its values cannot silently diverge
    /// from the declared schema. The default records that error.
    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        let _ = labels;
        schema.record_error(SchemaError::UndescribedEntry {
            name: name.to_owned(),
        });
    }

    /// Collects this entry under `name` with inherited constant `labels`.
    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues);

    /// Runs off-hot-path maintenance for this entry.
    fn housekeep(&self, context: &C);

    /// Whether [`housekeep`](MetricEntry::housekeep) has work to do for this
    /// entry. Defaults to `false` (entries with no upkeep, e.g. scalars, need
    /// not override it); entries that forward to a maintainable metric or a
    /// nested view mirror their [`housekeep`](MetricEntry::housekeep) routing.
    ///
    /// This feeds [`MetricTreeView::needs_housekeep_entries`], whose consumers
    /// gate whole *trees*: a derived tree's `#[metric(view)]` seam ORs it into
    /// the tree's `needs_housekeep`, which the registry's metric-tree seam and
    /// [`MetricTree::encode`](crate::MetricTree::encode) check before running
    /// a housekeep at all. The housekeep *walk* itself does not re-check it
    /// per entry -- see
    /// [`MetricTreeView::housekeep_entries`] for why.
    ///
    /// [`MetricTreeView::needs_housekeep_entries`]: crate::MetricTreeView::needs_housekeep_entries
    /// [`MetricTreeView::housekeep_entries`]: crate::MetricTreeView::housekeep_entries
    fn needs_housekeep(&self, _context: &C) -> bool {
        false
    }
}

/// Generates the chainable metadata setters shared by every builder and entry
/// that owns an [`EntryMetadata`]. `help_unit` is offered everywhere; `label`
/// only where the entry's schema honors declared dynamic labels. Tree-shaped
/// entries (metric trees and view mounts) own their label declarations
/// internally, so offering `label` there would silently drop the declared
/// names from the schema.
macro_rules! meta_setters {
    (help_unit) => {
        /// Attaches OpenMetrics `# HELP` text to this entry.
        pub fn help(mut self, help: impl Into<$crate::Help>) -> Self {
            self.metadata.help = Some(help.into());
            self
        }

        /// Attaches an OpenMetrics `# UNIT` value to this entry.
        pub fn unit(mut self, unit: impl Into<$crate::Unit>) -> Self {
            self.metadata.unit = Some(unit.into());
            self
        }
    };
    (label) => {
        /// Declares an additional dynamic label name for this entry.
        pub fn label(mut self, label: impl Into<$crate::LabelName>) -> Self {
            self.metadata.labels.push(label.into());
            self
        }
    };
}

/// A metric reference resolved against a runtime context — the read-only,
/// borrow-on-demand dual of [`std::borrow::Cow`].
///
/// An entry obtains its target either as a [`Ref`](Lens::Ref) it already holds,
/// or by [`Projection`](Lens::Projection) — focusing it out of the runtime
/// context with a selector applied at scrape time. Where `Cow` clones to *own*
/// on demand, a `Lens` *borrows* on demand. (Structurally it is the by-reference
/// analogue of leptos's `MaybeSignal { Static, Dynamic }`.)
///
/// This is why there isn't a single closure form: a held `&'a M` cannot be
/// expressed as a `for<'ctx> Fn(&'ctx C) -> &'ctx M` (its borrow has a fixed
/// lifetime, not a universally-quantified one), so the direct borrow is kept as
/// a plain field and only the selector is boxed. Internal impl detail; the
/// public builders (`entry::counter(name).source(m)` / `.select(f)`, and the
/// tree equivalents) construct it and entries hold it privately.
///
/// The boxed selector is `Send + Sync` so entries — and through them a whole
/// `MetricTreeView<'static, C>` — can be shared across threads or cached in a
/// `static`.
///
/// Both scalar/metric-tree leaf entries and nested-view tree entries resolve
/// their target — a metric, or a child context — through this one type.
pub(crate) enum Lens<'a, C, M: ?Sized> {
    /// A target the registry holds directly; the context is ignored.
    Ref(&'a M),
    /// A target focused out of the runtime context when accessed.
    Projection(Box<dyn for<'ctx> Fn(&'ctx C) -> &'ctx M + Send + Sync + 'a>),
}

impl<C, M: ?Sized> Lens<'_, C, M> {
    /// Resolves the target reference against `ctx`.
    #[inline]
    pub(crate) fn resolve<'c>(&'c self, ctx: &'c C) -> &'c M {
        match self {
            Lens::Ref(target) => target,
            Lens::Projection(select) => select(ctx),
        }
    }
}

impl<'a, C, M: ?Sized> From<&'a M> for Lens<'a, C, M> {
    fn from(target: &'a M) -> Self {
        Lens::Ref(target)
    }
}

pub mod kind;

mod leaf;
mod opaque;
mod tree;

pub use leaf::{
    EntryBuilder, LensEntry, ValueEntry, counter, counter_value, gauge, gauge_value, info, metric,
};
pub use tree::{
    Emitter, FamilyEmitter, FamilyViewEntry, TreeBuilder, TreeEntry, TreeViewBuilder, family_by,
    family_view, tree,
};

pub(crate) use opaque::OpaqueEntry;
