//! Nested metric-view entries: static subtrees and the dynamic per-member
//! `each` group. These compose a child [`MetricTreeView`] rather than borrowing
//! a single metric. A subtree resolves its child context through the shared
//! `Lens` — sourced directly or selected from the runtime context — just as a
//! leaf entry resolves its metric.

use super::{EntryMetadata, Lens, MetricEntry};
use crate::labels::slices::with_labels;
use crate::registry::MetricTreeView;
use crate::schema::MetricSchema;
use crate::values::MetricValues;
use crate::{LabelName, Name};
use std::marker::PhantomData;

/// Start builder for a nested metric view entry.
#[derive(Clone, Debug)]
pub struct TreeBuilder {
    metadata: EntryMetadata,
}

/// A nested metric view awaiting its child view. Its child context is sourced
/// directly or selected from the runtime context;
/// [`view`](TreeViewBuilder::view) attaches the child [`MetricTreeView`].
pub struct TreeViewBuilder<'a, C, D> {
    metadata: EntryMetadata,
    source: Lens<'a, C, D>,
}

/// A nested metric view entry: a child [`MetricTreeView`] over a context that is
/// either sourced directly or selected from the runtime context.
pub struct TreeEntry<'a, C, D> {
    metadata: EntryMetadata,
    source: Lens<'a, C, D>,
    tree: MetricTreeView<'a, D>,
}

// A nested view owns its label declarations, so the tree builder/entry
// deliberately have no `label` setter (it would be dropped from the schema).
impl_inherent_meta!(@meta_only TreeBuilder);
impl_inherent_meta!(@meta_only TreeViewBuilder<'a, C, D>);
impl_inherent_meta!(@meta_only TreeEntry<'a, C, D>);

impl TreeBuilder {
    /// Uses a direct child context source for a root [`crate::Registry`].
    pub fn source<D>(self, source: &D) -> TreeViewBuilder<'_, (), D> {
        TreeViewBuilder {
            metadata: self.metadata,
            source: source.into(),
        }
    }

    /// Selects a child context for a nested metric view.
    pub fn select<'f, C, D, F>(self, select: F) -> TreeViewBuilder<'f, C, D>
    where
        F: for<'ctx> Fn(&'ctx C) -> &'ctx D + 'f,
    {
        TreeViewBuilder {
            metadata: self.metadata,
            source: Lens::Projection(Box::new(select)),
        }
    }
}

impl<'a, C, D> TreeViewBuilder<'a, C, D> {
    /// Attaches the child metric view for this nested entry.
    pub fn view(self, tree: MetricTreeView<'a, D>) -> TreeEntry<'a, C, D> {
        TreeEntry {
            metadata: self.metadata,
            source: self.source,
            tree,
        }
    }
}

/// Starts a nested metric view entry builder.
pub fn tree(name: impl Into<Name>) -> TreeBuilder {
    TreeBuilder {
        metadata: EntryMetadata::new(name),
    }
}

impl<'a, C, D> MetricEntry<C> for TreeEntry<'a, C, D>
where
    D: 'a,
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
        self.tree
            .describe_prefixed(self.source.resolve(context), Some(name), labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.tree
            .describe_schema_prefixed(Some(name), labels, schema);
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.tree
            .collect_prefixed(self.source.resolve(context), Some(name), labels, values);
    }

    fn housekeep(&self, context: &C) {
        self.tree.housekeep_entries(self.source.resolve(context));
    }

    fn needs_housekeep(&self, context: &C) -> bool {
        self.tree
            .needs_housekeep_entries(self.source.resolve(context))
    }
}

/// The sink handed to an [`each`](crate::MetricTreeView::each) closure. Call
/// [`emit`](Emitter::emit) once per live element, passing the element and the
/// value its label should take.
pub struct Emitter<'a, D> {
    emit: &'a mut dyn FnMut(&str, &D),
}

impl<D> Emitter<'_, D> {
    /// Records `element`'s metrics under the label value `key`.
    pub fn emit(&mut self, key: &str, element: &D) {
        (self.emit)(key, element);
    }
}

/// One dynamic group of per-member [`MetricTreeView`]s: each live member
/// contributes its metrics with `label` set to its key.
pub struct EachEntry<'a, C, D, F> {
    metadata: EntryMetadata,
    label: LabelName,
    element_view: MetricTreeView<'a, D>,
    iterate: F,
    _types: PhantomData<fn(&C)>,
}

/// Starts a dynamic per-member entry. See [`crate::MetricTreeView::each`].
pub fn each<'a, C, D, F>(
    label: impl Into<LabelName>,
    element_view: MetricTreeView<'a, D>,
    iterate: F,
) -> EachEntry<'a, C, D, F>
where
    F: Fn(&C, &mut Emitter<'_, D>) + 'a,
{
    // A group's schema comes from the element view alone, so an element view
    // whose every entry needs a runtime projection would emit values with no
    // schema at all. Catch that wiring bug at registration in debug builds;
    // a partially-described view is surfaced by `MetricSchema::validate`.
    #[cfg(debug_assertions)]
    {
        let mut schema = MetricSchema::new();
        element_view.describe_schema_prefixed(None, &[], &mut schema);
        debug_assert!(
            !(schema.families().is_empty() && element_view.has_entries()),
            "an `each` element view declares no context-free schema: projected \
             `metric(...).select(...)` trees cannot describe themselves without a live \
             member; declare typed entries (counter/gauge/info/...) or a directly-held tree",
        );
    }
    EachEntry {
        // Empty name: member families sit at the enclosing view's prefix,
        // distinguished only by the key label.
        metadata: EntryMetadata::new(""),
        label: label.into(),
        element_view,
        iterate,
        _types: PhantomData,
    }
}

impl<C, D, F> MetricEntry<C> for EachEntry<'_, C, D, F>
where
    F: Fn(&C, &mut Emitter<'_, D>),
{
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(&self, _: &C, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.describe_schema(name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        // The schema is context-free: describe the element view's family shape
        // once, with the key label declared (value-less), rather than walking
        // whichever members happen to be live. An empty group still advertises
        // its families, and membership churn cannot change the scrape's shape.
        let element_labels = with_labels(labels, [(self.label.as_str(), "")]);
        self.element_view
            .describe_schema_prefixed(Some(name), &element_labels, schema);
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let label = self.label.as_str();
        let view = &self.element_view;
        let mut sink = |key: &str, element: &D| {
            let element_labels = with_labels(labels, [(label, key)]);
            view.collect_prefixed(element, Some(name), &element_labels, values);
        };
        (self.iterate)(context, &mut Emitter { emit: &mut sink });
    }

    fn housekeep(&self, context: &C) {
        let view = &self.element_view;
        let mut sink = |_key: &str, element: &D| {
            view.housekeep_entries(element);
        };
        (self.iterate)(context, &mut Emitter { emit: &mut sink });
    }

    fn needs_housekeep(&self, context: &C) -> bool {
        let view = &self.element_view;
        let mut needs = false;
        let mut sink = |_key: &str, element: &D| {
            needs = needs || view.needs_housekeep_entries(element);
        };
        (self.iterate)(context, &mut Emitter { emit: &mut sink });
        needs
    }
}
