//! Nested metric-view entries: static subtrees and the dynamic per-member
//! family-view group. These compose a child [`MetricTreeView`] rather than
//! borrowing a single metric. A subtree resolves its child context through the
//! shared `Lens` — sourced directly or selected from the runtime context — just
//! as a leaf entry resolves its metric.

use super::{EntryMetadata, Lens, MetricEntry};
use crate::labels::family::{LabelSet, normalized_label_names};
use crate::labels::slices::with_labels;
use crate::registry::MetricTreeView;
use crate::schema::MetricSchema;
use crate::values::MetricValues;
use crate::{LabelName, Name};
use std::marker::PhantomData;

/// Start builder for a nested metric view entry.
#[derive(Clone, Debug)]
#[must_use = "an entry builder does nothing until the entry is registered"]
pub struct TreeBuilder {
    metadata: EntryMetadata,
}

/// A nested metric view awaiting its child view. Its child context is sourced
/// directly or selected from the runtime context;
/// [`view`](TreeViewBuilder::view) attaches the child [`MetricTreeView`].
#[must_use = "an entry builder does nothing until the entry is registered"]
pub struct TreeViewBuilder<'a, C, D> {
    metadata: EntryMetadata,
    source: Lens<'a, C, D>,
}

/// A nested metric view entry: a child [`MetricTreeView`] over a context that is
/// either sourced directly or selected from the runtime context.
#[must_use = "a metric entry does nothing until registered"]
pub struct TreeEntry<'a, C, D> {
    metadata: EntryMetadata,
    source: Lens<'a, C, D>,
    tree: MetricTreeView<'a, D>,
}

// A nested view owns its label declarations, so the tree builder/entry
// deliberately have no `label` setter (it would be dropped from the schema).
impl TreeBuilder {
    meta_setters!(help_unit);

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
        F: for<'ctx> Fn(&'ctx C) -> &'ctx D + Send + Sync + 'f,
    {
        TreeViewBuilder {
            metadata: self.metadata,
            source: Lens::Projection(Box::new(select)),
        }
    }
}

impl<'a, C, D> TreeViewBuilder<'a, C, D> {
    meta_setters!(help_unit);

    /// Attaches the child metric view for this nested entry.
    pub fn view(self, tree: MetricTreeView<'a, D>) -> TreeEntry<'a, C, D> {
        TreeEntry {
            metadata: self.metadata,
            source: self.source,
            tree,
        }
    }
}

impl<C, D> TreeEntry<'_, C, D> {
    meta_setters!(help_unit);
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

/// The internal member seam below both emitters: one call is one member
/// plus its **ready** `(name, value)` labels. Everything downstream of
/// that -- extending the enclosing label slice with the labels and dispatching
/// the entry's current operation to the member's element view -- lives here,
/// once, whichever key form produced the labels: [`FamilyEmitter`] materializes
/// a typed key's labels to call it, [`Emitter`] stamps its one label on the
/// stack, borrowed.
struct MemberSink<'a, D> {
    element_view: &'a MetricTreeView<'a, D>,
    op: MemberOp<'a>,
}

/// The scrape operation a [`MemberSink`] dispatches per member. The entry
/// constructs one sink per [`MetricEntry`] method call, so the mode switch
/// stays with the entry's methods, exactly as before the seam moved.
enum MemberOp<'a> {
    /// Collect each member's values under the extended label slice.
    Collect {
        name: &'a str,
        labels: &'a [(&'a str, &'a str)],
        values: &'a mut MetricValues,
    },
    /// Run maintenance inside each member. Dispatched per emitted member
    /// without a per-member `needs_housekeep_entries` pre-check: that check
    /// walks the same element view the housekeep walks, so gating here would
    /// double the traversal for a member with pending upkeep and save nothing
    /// for a clean one (the element view's leaves already gate themselves; see
    /// [`MetricTreeView::housekeep_entries`]). The whole group is skipped when
    /// clean via [`FamilyViewEntry`]'s `needs_housekeep`, one level up.
    Housekeep,
    /// OR each member's pending-maintenance flag into `needs`.
    NeedsHousekeep { needs: &'a mut bool },
}

impl<'s, D> MemberSink<'s, D> {
    /// One member visit: applies the entry's current operation to `element`,
    /// with `member_labels` as the key's labels (only the collect walk reads
    /// them). The `'s: 'p` bound lets the enclosing label slice shorten to
    /// the caller's label lifetime, so both coexist in one slice.
    fn emit<'p>(&mut self, member_labels: impl IntoIterator<Item = (&'p str, &'p str)>, element: &D)
    where
        's: 'p,
    {
        match &mut self.op {
            MemberOp::Collect {
                name,
                labels,
                values,
            } => {
                let element_labels = with_labels(labels, member_labels);
                self.element_view
                    .collect_prefixed(element, Some(name), &element_labels, values);
            }
            MemberOp::Housekeep => self.element_view.housekeep_entries(element),
            MemberOp::NeedsHousekeep { needs } => {
                **needs = **needs || self.element_view.needs_housekeep_entries(element);
            }
        }
    }

    /// Whether the current operation reads the key's labels -- only the
    /// collect walk does -- so [`FamilyEmitter`] can skip materializing a
    /// typed key on maintenance walks, as the per-mode closures this seam
    /// replaced did.
    fn wants_labels(&self) -> bool {
        matches!(self.op, MemberOp::Collect { .. })
    }

    /// Reborrows this sink at a shorter lifetime, so [`family_by`] can rewrap
    /// the sink one emitter holds into the other emitter form.
    fn reborrow(&mut self) -> MemberSink<'_, D> {
        MemberSink {
            element_view: self.element_view,
            op: match &mut self.op {
                MemberOp::Collect {
                    name,
                    labels,
                    values,
                } => MemberOp::Collect {
                    name,
                    labels,
                    values,
                },
                MemberOp::Housekeep => MemberOp::Housekeep,
                MemberOp::NeedsHousekeep { needs } => MemberOp::NeedsHousekeep { needs },
            },
        }
    }
}

/// The sink handed to a [`family_view`](crate::MetricTreeView::family_view)
/// closure. Call [`emit`](FamilyEmitter::emit) once per live member, passing
/// the typed [`LabelSet`] key identifying it and the member itself.
///
/// The single-string-key sugar, [`family_by`](crate::MetricTreeView::family_by),
/// hands its closure an [`Emitter`] instead.
pub struct FamilyEmitter<'a, L, D> {
    sink: MemberSink<'a, D>,
    _key: PhantomData<fn(&L)>,
}

impl<L: LabelSet, D> FamilyEmitter<'_, L, D> {
    /// Records `element`'s metrics under `key`'s labels.
    pub fn emit(&mut self, key: &L, element: &D) {
        if !self.sink.wants_labels() {
            return self.sink.emit(std::iter::empty(), element);
        }
        // The sink needs the key's pairs to coexist as one slice, but the
        // `for_each_label` visitor only lends them one call at a time --
        // the same documented materializing trade-off as `Family::collect`.
        let mut pairs = Vec::new();
        key.encode_labels(&mut pairs);
        self.sink.emit(
            pairs
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
            element,
        );
    }
}

/// The sink handed to a [`family_by`](crate::MetricTreeView::family_by)
/// closure. Call [`emit`](Emitter::emit) once per live member, passing the
/// member and the value its label should take.
///
/// The key is a single string label value; when members are identified by a
/// typed, multi-label [`LabelSet`], use
/// [`family_view`](crate::MetricTreeView::family_view) and its
/// [`FamilyEmitter`] instead.
pub struct Emitter<'a, D> {
    /// The group's one declared label name.
    label: &'a str,
    sink: MemberSink<'a, D>,
}

impl<D> Emitter<'_, D> {
    /// Records `element`'s metrics under the label value `key`.
    pub fn emit(&mut self, key: &str, element: &D) {
        // One string key means the pair slice lives on the stack: the label
        // name and the caller's key are stamped borrowed, no copies.
        self.sink.emit([(self.label, key)], element);
    }
}

/// One dynamic group of per-member [`MetricTreeView`]s keyed by a typed
/// [`LabelSet`]: each live member contributes its metrics with the key's
/// label pairs applied. The **borrowed** dual of [`crate::Family`], and the
/// one implementation of the borrowed-family scrape semantics --
/// [`family_by`] layers the single-string-key form on top of it.
#[must_use = "a metric entry does nothing until registered"]
pub struct FamilyViewEntry<'a, C, L, D, F> {
    metadata: EntryMetadata,
    /// The group's declared label names, normalized (sorted, deduplicated)
    /// exactly as [`crate::Family`] stores its own `label_names`. Held as
    /// **data** rather than re-queried from `L`, so a group whose key type
    /// declares no [`LabelSet::label_names`] -- [`family_by`]'s label name is
    /// runtime data, not a type -- can still declare its context-free schema.
    label_names: Vec<String>,
    element_view: MetricTreeView<'a, D>,
    iterate: F,
    _types: PhantomData<fn(&C, &L)>,
}

/// Starts a dynamic per-member family view entry keyed by a typed
/// [`LabelSet`]. See [`crate::MetricTreeView::family_view`].
pub fn family_view<'a, C, L, D, F>(
    element_view: MetricTreeView<'a, D>,
    iterate: F,
) -> FamilyViewEntry<'a, C, L, D, F>
where
    L: LabelSet + 'a,
    F: Fn(&C, &mut FamilyEmitter<'_, L, D>) + 'a,
{
    let mut label_names = Vec::new();
    L::label_names(&mut label_names);
    let label_names = normalized_label_names(label_names);
    // The key's label names are part of the group's context-free schema, so
    // the label set type must declare them (`#[derive(LabelSet)]` does).
    // Fully-dynamic sets such as `Vec<(String, String)>` cannot.
    debug_assert!(
        !label_names.is_empty(),
        "`family_view` requires a label set type that declares its label names via \
         `LabelSet::label_names` (derived label sets do); a fully-dynamic label set \
         cannot declare the group's context-free schema",
    );
    family_view_with_names(label_names, element_view, iterate)
}

/// The one constructor behind [`family_view`] and [`family_by`]: the declared
/// label names arrive as data, which is what lets `family_by` declare its one
/// label explicitly for a key type whose `label_names()` is empty.
fn family_view_with_names<'a, C, L, D, F>(
    label_names: Vec<String>,
    element_view: MetricTreeView<'a, D>,
    iterate: F,
) -> FamilyViewEntry<'a, C, L, D, F>
where
    L: LabelSet + 'a,
    F: Fn(&C, &mut FamilyEmitter<'_, L, D>) + 'a,
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
            "a family-view element view declares no context-free schema: projected \
             `metric(...).select(...)` trees cannot describe themselves without a live \
             member; declare typed entries (counter/gauge/info/...) or a directly-held tree",
        );
    }
    FamilyViewEntry {
        // Empty name: member families sit at the enclosing view's prefix,
        // distinguished only by the key's label pairs.
        metadata: EntryMetadata::new(""),
        label_names,
        element_view,
        iterate,
        _types: PhantomData,
    }
}

/// Starts a dynamic per-member family view entry keyed by **one string
/// label**. See [`crate::MetricTreeView::family_by`].
///
/// `family_by` is [`family_view`] for the common one-string-key case: it
/// wraps `iterate` so each [`Emitter::emit`]`(key, element)` feeds the same
/// pair-level emission seam as a typed key, passing `[label]` as the group's
/// declared label names. The one `(label, key)` pair is built on the stack
/// per emit and stamped **borrowed** from the caller's `&str` -- unlike a
/// typed [`LabelSet`] key, whose pairs are materialized because
/// `for_each_label` may lend per-call temporaries.
///
/// The key type slot is `()` here: the label *name* is runtime data, not
/// part of a key type, which is exactly why [`FamilyViewEntry`] stores its
/// declared names as data -- `family_by` passes `[label]` explicitly.
// The return type spells out the layering (a `FamilyViewEntry` whose wrapped
// closure bridges the emitter forms); an alias cannot carry the `impl Fn` on
// stable.
#[allow(clippy::type_complexity)]
pub fn family_by<'a, C, D, F>(
    label: impl Into<LabelName>,
    element_view: MetricTreeView<'a, D>,
    iterate: F,
) -> FamilyViewEntry<'a, C, (), D, impl Fn(&C, &mut FamilyEmitter<'_, (), D>) + Send + Sync + 'a>
where
    D: 'a,
    F: Fn(&C, &mut Emitter<'_, D>) + Send + Sync + 'a,
{
    let label = label.into();
    let label_names = normalized_label_names([label.as_str()]);
    let iterate = move |context: &C, out: &mut FamilyEmitter<'_, (), D>| {
        // Same sink, other adapter: no key is ever built on this path.
        let mut emitter = Emitter {
            label: label.as_str(),
            sink: out.sink.reborrow(),
        };
        iterate(context, &mut emitter);
    };
    family_view_with_names(label_names, element_view, iterate)
}

impl<C, L, D, F> MetricEntry<C> for FamilyViewEntry<'_, C, L, D, F>
where
    L: LabelSet,
    F: Fn(&C, &mut FamilyEmitter<'_, L, D>),
{
    fn metadata(&self) -> &EntryMetadata {
        &self.metadata
    }

    fn describe(&self, _: &C, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.describe_schema(name, labels, schema);
    }

    fn describe_schema(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        // The schema is context-free: the element view's family shape is
        // described once, with the group's declared label names value-less,
        // rather than walking whichever members happen to be live. An empty
        // group still advertises its families, and membership churn cannot
        // change the scrape's shape.
        let element_labels = with_labels(
            labels,
            self.label_names.iter().map(|label| (label.as_str(), "")),
        );
        self.element_view
            .describe_schema_prefixed(Some(name), &element_labels, schema);
    }

    fn collect(&self, context: &C, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let mut out = FamilyEmitter {
            sink: MemberSink {
                element_view: &self.element_view,
                op: MemberOp::Collect {
                    name,
                    labels,
                    values,
                },
            },
            _key: PhantomData,
        };
        (self.iterate)(context, &mut out);
    }

    fn housekeep(&self, context: &C) {
        let mut out = FamilyEmitter {
            sink: MemberSink {
                element_view: &self.element_view,
                op: MemberOp::Housekeep,
            },
            _key: PhantomData,
        };
        (self.iterate)(context, &mut out);
    }

    fn needs_housekeep(&self, context: &C) -> bool {
        let mut needs = false;
        let mut out = FamilyEmitter {
            sink: MemberSink {
                element_view: &self.element_view,
                op: MemberOp::NeedsHousekeep { needs: &mut needs },
            },
            _key: PhantomData,
        };
        (self.iterate)(context, &mut out);
        needs
    }
}
