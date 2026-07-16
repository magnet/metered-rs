//! Context-aware metric views that compose metrics into one document.
//!
//! A [`MetricTreeView`] stores self-describing entries: each entry owns its
//! name, optional metadata, and the source/collector needed to describe and
//! collect values. [`Registry`](super::Registry) is a thin wrapper over a
//! `MetricTreeView<()>`.

use super::entry;
use crate::labels::slices::with_labels;
use crate::metric_tree::join_name;
use crate::schema::MetricSchema;
use crate::sink::MetricSink;
use crate::values::MetricValues;
use crate::{MetricTree, Name};
use std::fmt;

/// A type that describes its own metric view.
///
/// Implement it on a component so it owns the layout of its metrics, then
/// compose it into a parent with [`MetricTreeView::subtree`] -- no free
/// `*_view()` functions, and the parent never restates the child's metrics.
///
/// ```
/// use metered::entry::counter;
/// use metered::{MetricsView, MetricTreeView};
/// use std::sync::atomic::AtomicU64;
///
/// struct Worker {
///     runs: AtomicU64,
/// }
///
/// impl MetricsView for Worker {
///     fn metrics_view() -> MetricTreeView<'static, Self> {
///         let mut view = MetricTreeView::new();
///         view.register(counter("runs").select(|w: &Worker| &w.runs).help("Worker runs"));
///         view
///     }
/// }
/// ```
pub trait MetricsView: Sized {
    /// The metric view for this component.
    fn metrics_view() -> MetricTreeView<'static, Self>;
}

/// A reusable, context-aware metric view backed by self-describing entries.
///
/// Each entry owns its name, optional metadata, and the source/collector needed
/// to describe and collect values. The view borrows metrics from the supplied
/// context at schema/value/encode time.
pub struct MetricTreeView<'a, C> {
    /// Optional prefix applied to every entry name.
    pub(crate) prefix: Option<Name>,
    /// Constant labels applied to every series in this view.
    labels: Vec<(String, String)>,
    /// Registered metric entries.
    entries: Vec<Box<dyn entry::MetricEntry<C> + 'a>>,
    /// Whether scraping first runs entry maintenance.
    housekeep_on_scrape: bool,
}

impl<C> Default for MetricTreeView<'_, C> {
    fn default() -> Self {
        MetricTreeView {
            prefix: None,
            labels: Vec::new(),
            entries: Vec::new(),
            housekeep_on_scrape: true,
        }
    }
}

impl<'a, C: 'a> MetricTreeView<'a, C> {
    /// Creates an empty registry view.
    pub fn new() -> Self {
        MetricTreeView::default()
    }

    /// Creates a registry view whose metric names are prefixed with `prefix`.
    pub fn with_prefix(prefix: impl Into<Name>) -> Self {
        MetricTreeView {
            prefix: Some(prefix.into()),
            ..Default::default()
        }
    }

    /// Adds a constant label applied to every series in this registry view.
    pub fn label(&mut self, name: impl Into<String>, value: impl Into<String>) -> &mut Self {
        self.labels.push((name.into(), value.into()));
        self
    }

    /// Registers one self-describing context-aware entry.
    pub fn register<E>(&mut self, entry: E) -> &mut Self
    where
        E: entry::MetricEntry<C> + 'a,
    {
        self.entries.push(Box::new(entry));
        self
    }

    /// Registers a metric tree selected from the runtime context **without** a
    /// name segment: its metrics sit directly at this view's prefix. Use it to
    /// splice in a self-naming bundle (e.g. a `metered-tracing` layer).
    pub fn flatten<M, F>(&mut self, select: F) -> &mut Self
    where
        M: MetricTree + 'static,
        F: for<'ctx> Fn(&'ctx C) -> &'ctx M + 'a,
    {
        self.register(entry::metric(Name::literal("")).select(select))
    }

    /// Registers a nested metric tree view under `name`.
    ///
    /// `name` supplies the child's only name segment, so the child view should
    /// not also carry a matching prefix -- there is no hidden "prefix equals
    /// mount name" dedup, and a self-prefixed child would double the segment.
    /// To splice a self-naming/self-prefixed bundle in without a segment, use
    /// [`flatten`](Self::flatten) instead.
    pub fn tree<D>(
        &mut self,
        name: impl Into<Name>,
        select: impl for<'ctx> Fn(&'ctx C) -> &'ctx D + 'a,
        tree: MetricTreeView<'a, D>,
    ) -> &mut Self
    where
        D: 'a,
    {
        self.register(entry::tree(name).select(select).view(tree))
    }

    /// Nests a child component's own [`MetricsView`] under `name`.
    ///
    /// `D` is inferred from the selector, so this is `tree(name, select,
    /// D::metrics_view())` without naming the child view explicitly:
    /// `view.subtree("jobs", |app: &App| &app.jobs)`.
    pub fn subtree<D, F>(&mut self, name: impl Into<Name>, select: F) -> &mut Self
    where
        D: MetricsView + 'a,
        F: for<'ctx> Fn(&'ctx C) -> &'ctx D + 'a,
    {
        self.tree(name, select, D::metrics_view())
    }

    /// Fans out a **dynamic** set of sub-components as one labeled family group:
    /// for each live member, the member's `element_view` metrics are emitted with
    /// `label` set to its key. The dynamic analog of [`subtree`](Self::subtree).
    ///
    /// Note that the first argument is a **label name** (a dimension applied to
    /// every member's series), not a name segment in the metric path like
    /// [`subtree`](Self::subtree)'s `name`.
    ///
    /// `iterate` drives the **collect** walk and calls
    /// [`Emitter::emit`](crate::Emitter::emit) once per member, so the caller
    /// owns the lock/snapshot scope. The **schema** is context-free: it is
    /// described from `element_view` alone (with `label` declared), so the
    /// scrape's shape never depends on which members are live -- an empty group
    /// still advertises its families. For that reason the element view must
    /// declare its metrics through typed entries (or a directly-held tree): a
    /// context-projected sub-tree inside an element view cannot describe itself
    /// and is recorded as a [`crate::SchemaError::UndescribedEntry`] (surfaced
    /// by [`crate::MetricSchema::validate`]; an element view with *no*
    /// describable entry at all is a `debug_assert!` at registration). This is
    /// the tool for service-shaped state that is itself dynamic
    /// (a map of rails, remotes, shards, ...) where the metrics live *in* the
    /// members rather than in a central [`crate::Family`].
    ///
    /// ```
    /// # use metered::{MetricTreeView, MetricsView};
    /// # use std::collections::HashMap;
    /// # use std::sync::atomic::AtomicU64;
    /// # struct Rail { sent: AtomicU64 }
    /// # impl MetricsView for Rail {
    /// #     fn metrics_view() -> MetricTreeView<'static, Self> {
    /// #         let mut v = MetricTreeView::new();
    /// #         v.register(metered::entry::counter("sent").select(|r: &Rail| &r.sent));
    /// #         v
    /// #     }
    /// # }
    /// # struct Rails { map: HashMap<String, Rail> }
    /// let mut view = MetricTreeView::<Rails>::new();
    /// view.each("rail", Rail::metrics_view(), |rails: &Rails, out| {
    ///     for (name, rail) in &rails.map {
    ///         out.emit(name, rail);
    ///     }
    /// });
    /// ```
    pub fn each<D, F>(
        &mut self,
        label: impl Into<crate::LabelName>,
        element_view: MetricTreeView<'a, D>,
        iterate: F,
    ) -> &mut Self
    where
        D: 'a,
        F: Fn(&C, &mut entry::Emitter<'_, D>) + 'a,
    {
        self.register(entry::each(label, element_view, iterate))
    }

    /// Sets whether a scrape first runs [`housekeep`](MetricTreeView::housekeep).
    pub fn housekeep_on_scrape(&mut self, on: bool) -> &mut Self {
        self.housekeep_on_scrape = on;
        self
    }

    /// Runs off-hot-path maintenance over every selected metric.
    pub fn housekeep(&self, context: &C) {
        self.housekeep_entries(context);
    }

    #[doc(hidden)]
    pub fn housekeep_entries(&self, context: &C) {
        for entry in &self.entries {
            entry.housekeep(context);
        }
    }

    /// Whether any entry in this view currently needs
    /// [`housekeep`](MetricTreeView::housekeep). The read-only dual of
    /// [`housekeep_entries`](MetricTreeView::housekeep_entries): a derived tree's
    /// `#[metric(view)]` seam ORs this into its `needs_housekeep` so a view whose
    /// only maintainable metrics sit behind a view field is not skipped.
    #[doc(hidden)]
    pub fn needs_housekeep_entries(&self, context: &C) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.needs_housekeep(context))
    }

    /// Encodes all selected metrics into `sink`.
    pub fn encode(&self, context: &C, sink: &mut dyn MetricSink) -> fmt::Result {
        sink.encode_document(&self.schema(context), &self.values(context))
    }

    /// Describes the OpenMetrics families selected from `context`.
    pub fn schema(&self, context: &C) -> MetricSchema {
        let mut schema = MetricSchema::new();
        self.describe_prefixed(context, None, &[], &mut schema);
        schema
    }

    /// Samples the current values selected from `context`.
    pub fn values(&self, context: &C) -> MetricValues {
        if self.housekeep_on_scrape {
            self.housekeep_entries(context);
        }

        self.values_without_housekeep(context)
    }

    /// Samples the current values selected from `context` without first running
    /// entry maintenance. Use this when the caller has already driven
    /// [`housekeep`](MetricTreeView::housekeep) for this scrape cycle.
    pub fn values_without_housekeep(&self, context: &C) -> MetricValues {
        let mut values = MetricValues::new();
        self.collect_prefixed(context, None, &[], &mut values);
        values
    }

    #[doc(hidden)]
    pub fn describe_prefixed(
        &self,
        context: &C,
        parent_prefix: Option<&str>,
        inherited_labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        self.walk_entries(parent_prefix, inherited_labels, |entry, name, labels| {
            let metadata = entry.metadata();
            schema.set_metadata_for(name, metadata.help.clone(), metadata.unit.clone());
            entry.describe(context, name, labels, schema);
        });
    }

    /// Describes every entry's schema **without** a runtime context, so a
    /// dynamic group's shape is independent of live membership. See
    /// [`entry::MetricEntry::describe_schema`].
    #[doc(hidden)]
    pub fn describe_schema_prefixed(
        &self,
        parent_prefix: Option<&str>,
        inherited_labels: &[(&str, &str)],
        schema: &mut MetricSchema,
    ) {
        self.walk_entries(parent_prefix, inherited_labels, |entry, name, labels| {
            let metadata = entry.metadata();
            schema.set_metadata_for(name, metadata.help.clone(), metadata.unit.clone());
            entry.describe_schema(name, labels, schema);
        });
    }

    #[doc(hidden)]
    pub fn collect_prefixed(
        &self,
        context: &C,
        parent_prefix: Option<&str>,
        inherited_labels: &[(&str, &str)],
        values: &mut MetricValues,
    ) {
        self.walk_entries(parent_prefix, inherited_labels, |entry, name, labels| {
            entry.collect(context, name, labels, values);
        });
    }

    pub(crate) fn has_entries(&self) -> bool {
        !self.entries.is_empty()
    }

    /// The one per-entry walk shared by the describe/collect traversals:
    /// resolves this view's combined prefix and constant labels, then calls
    /// `per_entry` with each entry's full family name and label set.
    fn walk_entries(
        &self,
        parent_prefix: Option<&str>,
        inherited_labels: &[(&str, &str)],
        mut per_entry: impl FnMut(&dyn entry::MetricEntry<C>, &str, &[(&str, &str)]),
    ) {
        let const_labels = self.combined_labels(inherited_labels);
        let base_prefix = combined_prefix(parent_prefix, self.prefix.as_ref().map(Name::as_str));

        for entry in &self.entries {
            let full_name = full_name(&base_prefix, entry.metadata().name.as_str());
            per_entry(&**entry, &full_name, &const_labels);
        }
    }

    fn combined_labels<'b>(&'b self, inherited: &[(&'b str, &'b str)]) -> Vec<(&'b str, &'b str)> {
        with_labels(
            inherited,
            self.labels
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        )
    }
}

fn full_name(base_prefix: &Option<String>, name: &str) -> String {
    match base_prefix {
        // An empty entry name (a flattened sub-tree) contributes no segment, so
        // its metrics sit directly at the base prefix.
        Some(prefix) if name.is_empty() => prefix.clone(),
        Some(prefix) => join_name(prefix, name),
        None => name.to_owned(),
    }
}

fn combined_prefix(parent: Option<&str>, own: Option<&str>) -> Option<String> {
    match (parent, own) {
        (Some(parent), Some(own)) => Some(join_name(parent, own)),
        (Some(parent), None) => Some(parent.to_owned()),
        (None, Some(own)) => Some(own.to_owned()),
        (None, None) => None,
    }
}
