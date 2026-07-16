//! Labeled metric families: one metric per label set, for dynamic label
//! dimensions.
//!
//! A [`Family`] maps a label set to a metric, creating metrics on first use --
//! the analogue of a Prometheus metric family. Use it when a label dimension is
//! dynamic (e.g. one counter per `method`, per `route`, per `status`). It pairs
//! with the natural-structure model: use a newtype for fixed metrics and a
//! `Family` for dynamic label sets.
//!
//! Like the primitives, a `Family` is the owning component's source of truth and
//! is held in its state (no shared handle); the exposition borrows it.
//!
//! ```
//! use metered::{Counter, Family, MetricTree};
//! use metered_om::OpenMetricsEncoder;
//! use std::sync::atomic::AtomicU64;
//!
//! // Fully dynamic labels:
//! let by_method: Family<Vec<(String, String)>, AtomicU64> = Family::with_label_names(["method"]);
//! by_method.with(&vec![("method".to_owned(), "get".to_owned())], |c| c.incr());
//!
//! let mut buf = String::new();
//! {
//!     let mut enc = OpenMetricsEncoder::new(&mut buf);
//!     by_method.encode("requests", &[], &mut enc).unwrap();
//!     enc.finish().unwrap();
//! }
//! assert!(buf.contains("requests_total{method=\"get\"} 1"));
//! ```

use crate::labels::slices::with_labels;
use crate::metric_tree::MetricTree;
use crate::schema::MetricSchema;
use crate::values::MetricValues;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::OnceLock;

/// A set of label name/value pairs identifying one series within a [`Family`].
///
/// Implemented for `()` (no labels) and `Vec<(String, String)>` (fully dynamic).
/// Implement it for your own key type to get typed, validated label sets.
///
/// The one required method is the visitor,
/// [`for_each_label`](LabelSet::for_each_label): it lends each `(name, value)`
/// pair as `&str`, so an implementation whose labels already exist as strings
/// (owned, interned, `Arc<str>`, ...) encodes without allocating.
/// [`encode_labels`](LabelSet::encode_labels) is the materializing convenience
/// built on top of it, for sinks that want owned pairs.
pub trait LabelSet {
    /// Visits this set's `(name, value)` pairs in encoding order.
    ///
    /// The borrows are only valid for the duration of each call, which lets
    /// implementations lend computed values (e.g. a `Display` rendering) as
    /// well as stored ones.
    fn for_each_label(&self, f: &mut dyn FnMut(&str, &str));

    /// Appends owned copies of this set's `(name, value)` pairs to `out`.
    ///
    /// A materializing convenience over
    /// [`for_each_label`](LabelSet::for_each_label), for sinks that want owned
    /// pairs; prefer the visitor when a borrowed view suffices.
    fn encode_labels(&self, out: &mut Vec<(String, String)>) {
        self.for_each_label(&mut |name, value| out.push((name.to_owned(), value.to_owned())));
    }

    /// Appends label names known from the type alone.
    ///
    /// Fully dynamic label sets such as `Vec<(String, String)>` cannot know
    /// their shape statically; use [`Family::with_label_names`] for those.
    fn label_names(_out: &mut Vec<String>) {}
}

impl LabelSet for () {
    fn for_each_label(&self, _f: &mut dyn FnMut(&str, &str)) {}
}

impl LabelSet for Vec<(String, String)> {
    fn for_each_label(&self, f: &mut dyn FnMut(&str, &str)) {
        for (name, value) in self {
            f(name, value);
        }
    }
}

impl LabelSet for (&'static str, String) {
    fn for_each_label(&self, f: &mut dyn FnMut(&str, &str)) {
        f(self.0, &self.1);
    }
}

/// A constructor for the metric created when a new label set is first seen.
/// Blanket-implemented for `Fn() -> M`, and defaulted to `M::default` via
/// [`Family::default`].
pub trait MetricConstructor<M> {
    /// Creates a new metric instance.
    fn new_metric(&self) -> M;
}

impl<M, F: Fn() -> M> MetricConstructor<M> for F {
    fn new_metric(&self) -> M {
        self()
    }
}

/// A family of metrics of type `M`, one per label set `L`.
///
/// New metrics are created on first access via [`Family::with`], using the
/// family's constructor (`M::default` by default, or a custom one via
/// [`Family::new_with_constructor`] for typed label sets, or
/// [`Family::new_with_constructor_and_label_names`] for dynamic labels that
/// need an explicit schema -- useful for histograms with specific buckets).
///
/// # Whole metric structs per key
///
/// `M` is any [`MetricTree`], not just a scalar: a derived metric struct works
/// as-is, giving one whole bundle of metrics per key.
///
/// ```
/// use metered::{Counter, Family, Gauge, LabelSet, MetricTree};
/// use std::sync::atomic::{AtomicI64, AtomicU64};
///
/// #[derive(Clone, PartialEq, Eq, Hash, LabelSet)]
/// struct RailLabels {
///     rail: String,
/// }
///
/// #[derive(Default, MetricTree)]
/// struct RailMetrics {
///     #[metric(counter)]
///     sent: AtomicU64,
///     #[metric]
///     queue_depth: AtomicI64,
/// }
///
/// let rails: Family<RailLabels, RailMetrics> = Family::default();
/// rails.with(&RailLabels { rail: "sepa".to_owned() }, |m| {
///     Counter::incr(&m.sent);
///     Gauge::set(&m.queue_depth, 3);
/// });
/// ```
///
/// # Owned vs borrowed keyed members
///
/// A `Family` **owns** its keyed members: the map from label set to metrics
/// lives inside the family, and [`Family::with`] creates members on first use.
/// When the keyed state already lives in your own domain map -- a map of
/// rails, remotes, shards whose metrics sit *in* the members -- expose it
/// **borrowed** instead, through
/// [`MetricTreeView::family_view`](crate::MetricTreeView::family_view) (typed
/// [`LabelSet`] key) or its single-string-key sugar
/// [`MetricTreeView::family_by`](crate::MetricTreeView::family_by). The
/// borrowed family view is the same contract as `Family` with the storage
/// inverted, and the two render identically for the same logical data.
pub struct Family<L, M, C = fn() -> M> {
    metrics: RwLock<HashMap<L, M>>,
    constructor: C,
    label_names: Vec<String>,
    /// The metric `describe` falls back to while no series exists yet, built
    /// lazily and at most once (an empty family used to construct a throwaway
    /// metric on every scrape). It is only ever described -- never observed,
    /// collected, or maintained -- so it cannot leak phantom samples.
    describe_prototype: OnceLock<M>,
}

impl<L: LabelSet + Clone + Hash + Eq, M: Default> Default for Family<L, M> {
    fn default() -> Self {
        let mut label_names = Vec::new();
        L::label_names(&mut label_names);
        Family {
            metrics: RwLock::new(HashMap::new()),
            constructor: M::default,
            label_names,
            describe_prototype: OnceLock::new(),
        }
    }
}

impl<L: LabelSet + Clone + Hash + Eq, M, C: MetricConstructor<M>> Family<L, M, C> {
    /// Creates a family that builds new metrics with `constructor`.
    ///
    /// ```
    /// use metered::bucket_histogram::{BucketHistogram, Buckets};
    /// use metered::Family;
    ///
    /// let durations: Family<Vec<(String, String)>, BucketHistogram, _> =
    ///     Family::new_with_constructor_and_label_names(
    ///         || BucketHistogram::new(Buckets::fast_seconds()),
    ///         ["route"],
    ///     );
    /// ```
    pub fn new_with_constructor(constructor: C) -> Self {
        let mut label_names = Vec::new();
        L::label_names(&mut label_names);
        Family::new_with_constructor_and_label_names(constructor, label_names)
    }

    /// Creates a family that builds new metrics with `constructor` and declares
    /// the label names it can emit even before any series has been observed.
    pub fn new_with_constructor_and_label_names(
        constructor: C,
        label_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Family {
            metrics: RwLock::new(HashMap::new()),
            constructor,
            label_names: normalized_label_names(label_names),
            describe_prototype: OnceLock::new(),
        }
    }

    /// Accesses the metric for `labels`, creating it if absent, and runs `f`
    /// against it. The family is read-locked while `f` runs, so keep `f` short
    /// (e.g. a single `inc` / `observe`).
    pub fn with<R>(&self, labels: &L, f: impl FnOnce(&M) -> R) -> R {
        {
            let map = self.metrics.read();
            if let Some(metric) = map.get(labels) {
                return f(metric);
            }
        }
        let mut map = self.metrics.write();
        let metric = map
            .entry(labels.clone())
            .or_insert_with(|| self.constructor.new_metric());
        f(metric)
    }

    /// Accesses the metric for `labels` only when the series already exists,
    /// and runs `f` against it; returns `None` -- creating nothing -- when it
    /// does not.
    ///
    /// This is the non-minting counterpart of [`Family::with`], for callers
    /// that enforce their own series-admission policy (e.g. a series cap that
    /// folds novel keys into an overflow series instead of creating them).
    /// It only ever takes the read lock, so an existing series records at
    /// exactly the cost of [`Family::with`]'s fast path.
    pub fn with_existing<R>(&self, labels: &L, f: impl FnOnce(&M) -> R) -> Option<R> {
        self.metrics.read().get(labels).map(f)
    }

    /// Removes the metric for `labels`, returning `true` if one was present.
    /// Useful for dropping stale series (e.g. a closed connection).
    pub fn remove(&self, labels: &L) -> bool {
        self.metrics.write().remove(labels).is_some()
    }

    /// The number of distinct label sets currently tracked.
    pub fn len(&self) -> usize {
        self.metrics.read().len()
    }

    /// Returns `true` if no label sets are tracked yet.
    pub fn is_empty(&self) -> bool {
        self.metrics.read().is_empty()
    }

    /// Walks every tracked series in sorted label order, composing `labels`
    /// with each series' pairs (inner wins on name collision).
    ///
    /// Holds the family read lock for the duration of the walk. Callers that
    /// drive member collection themselves (e.g. with names joined once outside
    /// the per-series loop) use this instead of
    /// [`MetricTree::collect`](MetricTree::collect).
    pub fn for_each_series(
        &self,
        labels: &[(&str, &str)],
        mut visit: impl FnMut(&[(&str, &str)], &M),
    ) {
        let map = self.metrics.read();
        // Series render in sorted label order, so every key's pairs must
        // coexist before any metric is visited. The visitor cannot lend
        // borrows past each call, hence this one materializing use of
        // `encode_labels` in the scrape path.
        let mut series: Vec<(Vec<(String, String)>, &M)> = map
            .iter()
            .map(|(label_set, metric)| {
                let mut pairs = Vec::new();
                label_set.encode_labels(&mut pairs);
                (pairs, metric)
            })
            .collect();
        series.sort_by(|(a, _), (b, _)| a.cmp(b));

        for (pairs, metric) in series {
            let all = with_labels(labels, pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())));
            visit(&all, metric);
        }
    }

    /// Invokes `visit` once with the family's describe target (first live
    /// series, or the cached empty-family prototype) and the composed label
    /// set used for schema declaration.
    ///
    /// Holds the family read lock for the duration of `visit`. Callers that
    /// drive member describe themselves (e.g. with names joined once at the
    /// owning tree) use this instead of
    /// [`MetricTree::describe`](MetricTree::describe).
    pub fn with_describe_member(
        &self,
        labels: &[(&str, &str)],
        visit: impl FnOnce(&[(&str, &str)], &M),
    ) {
        let map = self.metrics.read();

        // Fall back to a live key's label names when none were declared, so
        // values never carry labels the schema does not mention.
        let mut fallback = Vec::new();
        if self.label_names.is_empty() {
            if let Some(key) = map.keys().next() {
                key.for_each_label(&mut |name, _| fallback.push(name.to_owned()));
                fallback.sort();
                fallback.dedup();
            }
        }
        let declared = if self.label_names.is_empty() {
            &fallback
        } else {
            &self.label_names
        };
        let all = with_labels(labels, declared.iter().map(|name| (name.as_str(), "")));

        if let Some(metric) = map.values().next() {
            visit(&all, metric);
        } else {
            // No live series to describe from: use the cached prototype (see
            // its field docs), built at most once across all scrapes rather
            // than a fresh throwaway metric per describe.
            let metric = self
                .describe_prototype
                .get_or_init(|| self.constructor.new_metric());
            visit(&all, metric);
        }
    }
}

impl<L: LabelSet + Clone + Hash + Eq, M> Family<L, M> {
    /// Creates a family that builds new metrics with a function pointer while
    /// keeping the family type as `Family<L, M>`.
    ///
    /// Use this when the metric has a custom constructor (for example a
    /// histogram with non-default buckets), but you do not want that constructor
    /// type to leak into structs that own the family.
    ///
    /// ```
    /// use metered::{BucketHistogram, Buckets, Family};
    ///
    /// fn fast_latency() -> BucketHistogram {
    ///     BucketHistogram::new(Buckets::fast_seconds())
    /// }
    ///
    /// let latencies: Family<(&'static str, String), BucketHistogram> =
    ///     Family::new_with_constructor_fn(fast_latency);
    /// ```
    pub fn new_with_constructor_fn(constructor: fn() -> M) -> Self {
        let mut label_names = Vec::new();
        L::label_names(&mut label_names);
        Family::new_with_constructor_and_label_names(constructor, label_names)
    }

    /// Creates a family that builds new metrics with a function pointer and
    /// declares label names up front.
    ///
    /// This is the function-pointer equivalent of
    /// [`Family::new_with_constructor_and_label_names`], useful when the label
    /// set itself cannot declare its shape.
    pub fn new_with_constructor_fn_and_label_names(
        constructor: fn() -> M,
        label_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Family::new_with_constructor_and_label_names(constructor, label_names)
    }
}

impl<L: Clone + Hash + Eq, M: Default> Family<L, M> {
    /// Creates a family with explicit label names, using `M::default` for new
    /// series. Use this for dynamic label sets such as `Vec<(String, String)>`.
    pub fn with_label_names(label_names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Family {
            metrics: RwLock::new(HashMap::new()),
            constructor: M::default,
            label_names: normalized_label_names(label_names),
            describe_prototype: OnceLock::new(),
        }
    }
}

/// # Schema label names
///
/// [`describe`](MetricTree::describe) declares the family's dynamic label
/// names from `label_names` -- populated statically by
/// [`LabelSet::label_names`] or explicitly via the `*_label_names`
/// constructors. Label sets that only know their names per value (e.g.
/// `(&'static str, String)`, whose name is a runtime `&'static str`) leave
/// `label_names` empty; in that case `describe` falls back to encoding the
/// label names from a live key, so the schema declares the labels its values
/// actually carry. The fallback needs at least one recorded series -- an
/// empty such family still describes no dynamic labels, so declare names
/// explicitly when the schema must be complete before traffic arrives.
impl<L, M, C> MetricTree for Family<L, M, C>
where
    L: LabelSet + Clone + Hash + Eq,
    M: MetricTree,
    C: MetricConstructor<M>,
{
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.with_describe_member(labels, |all, metric| {
            metric.describe(name, all, schema);
        });
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.for_each_series(labels, |all, metric| {
            metric.collect(name, all, values);
        });
    }

    fn housekeep(&self) {
        for metric in self.metrics.read().values() {
            // Per-member gate, mirroring the registry's metric-tree seam
            // (`LensKind::MetricTree::housekeep`): one member with pending
            // upkeep must not force maintenance across every other series.
            if metric.needs_housekeep() {
                metric.housekeep();
            }
        }
    }

    fn needs_housekeep(&self) -> bool {
        self.metrics
            .read()
            .values()
            .any(MetricTree::needs_housekeep)
    }
}

pub(crate) fn normalized_label_names(
    label_names: impl IntoIterator<Item = impl Into<String>>,
) -> Vec<String> {
    let mut label_names: Vec<String> = label_names.into_iter().map(Into::into).collect();
    label_names.sort();
    label_names.dedup();
    label_names
}

impl<L: fmt::Debug, M: fmt::Debug, C> fmt::Debug for Family<L, M, C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Family")
            .field("metrics", &*self.metrics.read())
            .field("label_names", &self.label_names)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{Counter, CounterSource};
    use std::sync::atomic::AtomicU64;

    // The OpenMetrics rendering of a `Family` (one `# TYPE`, many series) is
    // tested in `metered-om/tests/render_trees.rs`.

    #[test]
    fn a_visitor_only_impl_gets_encode_labels_for_free() {
        struct FixedLabels;

        impl LabelSet for FixedLabels {
            fn for_each_label(&self, f: &mut dyn FnMut(&str, &str)) {
                f("method", "get");
                f("status", "200");
            }
        }

        let mut pairs = Vec::new();
        FixedLabels.encode_labels(&mut pairs);
        assert_eq!(
            pairs,
            vec![
                ("method".to_owned(), "get".to_owned()),
                ("status".to_owned(), "200".to_owned()),
            ],
            "the provided encode_labels must materialize exactly what the visitor lends"
        );
    }

    #[test]
    fn with_existing_records_without_ever_minting_a_series() {
        let family: Family<(&'static str, String), AtomicU64> = Family::default();

        // An absent key runs nothing and creates nothing.
        assert_eq!(
            family.with_existing(&("conn", "a".to_owned()), |c| c.get()),
            None
        );
        assert!(family.is_empty(), "a miss must not mint the series");

        // A present key records exactly like `with`.
        family.with(&("conn", "a".to_owned()), |c| c.incr());
        assert_eq!(
            family.with_existing(&("conn", "a".to_owned()), |c| {
                c.incr();
                c.get()
            }),
            Some(2)
        );
        assert_eq!(family.len(), 1);
    }

    #[test]
    fn remove_drops_a_series() {
        let family: Family<(&'static str, String), std::sync::atomic::AtomicU64> =
            Family::default();
        family.with(&("conn", "a".to_owned()), |c| c.incr());
        assert_eq!(family.len(), 1);
        assert!(family.remove(&("conn", "a".to_owned())));
        assert!(family.is_empty());
    }

    #[test]
    fn describe_falls_back_to_a_live_keys_label_names() {
        use crate::metric_tree::MetricTree;

        // `(&'static str, String)` cannot declare its label name statically,
        // so without `with_label_names` the schema used to omit it while every
        // value still carried it. A live key now supplies the fallback.
        let family: Family<(&'static str, String), AtomicU64> = Family::default();
        family.with(&("method", "get".to_owned()), |c| c.incr());

        let mut schema = crate::schema::MetricSchema::new();
        family.describe("requests", &[], &mut schema);
        assert_eq!(
            schema.family("requests").unwrap().labels,
            vec!["method"],
            "the schema must declare the label the values carry"
        );
    }

    #[test]
    fn a_key_label_colliding_with_an_enclosing_constant_label_wins_once() {
        // A family key label named like an enclosing (registry/view) constant
        // label must render exactly one pair -- the inner (key) pair -- in
        // both the schema and the values, never a duplicated label name.
        let family: Family<(&'static str, String), AtomicU64> = Family::default();
        family.with(&("method", "get".to_owned()), |c| c.incr());

        let enclosing = [("service", "api"), ("method", "outer")];

        let mut schema = crate::schema::MetricSchema::new();
        family.describe("requests", &enclosing, &mut schema);
        assert_eq!(
            schema.family("requests").unwrap().labels,
            vec!["method", "service"],
            "the schema declares the colliding name exactly once"
        );
        assert!(
            schema.validate().is_ok(),
            "a resolved collision is not a schema error"
        );

        let mut values = MetricValues::new();
        family.collect("requests", &enclosing, &mut values);
        let sample = &values.samples()[0];
        assert_eq!(
            sample.labels,
            vec![
                ("service".to_owned(), "api".to_owned()),
                ("method".to_owned(), "get".to_owned()),
            ],
            "the inner (key) pair wins and the outer constant pair is dropped"
        );
    }

    #[test]
    fn housekeep_only_visits_members_that_need_it() {
        use std::sync::atomic::{AtomicBool, Ordering};

        #[derive(Default)]
        struct Upkeep {
            needs: AtomicBool,
            runs: AtomicU64,
        }

        impl MetricTree for Upkeep {
            fn describe(&self, _: &str, _: &[(&str, &str)], _: &mut crate::schema::MetricSchema) {}
            fn collect(&self, _: &str, _: &[(&str, &str)], _: &mut crate::values::MetricValues) {}
            fn housekeep(&self) {
                self.runs.fetch_add(1, Ordering::Relaxed);
            }
            fn needs_housekeep(&self) -> bool {
                self.needs.load(Ordering::Relaxed)
            }
        }

        let family: Family<(&'static str, String), Upkeep> = Family::default();
        family.with(&("shard", "dirty".to_owned()), |m| {
            m.needs.store(true, Ordering::Relaxed);
        });
        family.with(&("shard", "clean".to_owned()), |_| {});

        assert!(
            family.needs_housekeep(),
            "one dirty member flags the family"
        );
        family.housekeep();

        family.with(&("shard", "dirty".to_owned()), |m| {
            assert_eq!(m.runs.load(Ordering::Relaxed), 1, "the dirty member ran");
        });
        family.with(&("shard", "clean".to_owned()), |m| {
            assert_eq!(
                m.runs.load(Ordering::Relaxed),
                0,
                "the clean member's housekeep must not run"
            );
        });
    }

    #[test]
    fn empty_family_describe_builds_the_prototype_exactly_once() {
        use std::sync::Arc;
        use std::sync::atomic::Ordering;

        let constructions = Arc::new(AtomicU64::new(0));
        let counting = {
            let constructions = Arc::clone(&constructions);
            move || {
                constructions.fetch_add(1, Ordering::Relaxed);
                AtomicU64::new(0)
            }
        };
        let family: Family<(&'static str, String), AtomicU64, _> =
            Family::new_with_constructor_and_label_names(counting, ["method"]);

        for _ in 0..5 {
            let mut schema = crate::schema::MetricSchema::new();
            family.describe("requests", &[], &mut schema);
            assert_eq!(schema.family("requests").unwrap().labels, vec!["method"]);
        }
        assert_eq!(
            constructions.load(Ordering::Relaxed),
            1,
            "N describes of an empty family build exactly one prototype"
        );
        assert!(family.is_empty(), "the prototype never becomes a series");
    }

    #[test]
    fn function_constructor_does_not_leak_into_family_type() {
        fn seeded_counter() -> AtomicU64 {
            AtomicU64::new(41)
        }

        let family: Family<(&'static str, String), AtomicU64> =
            Family::new_with_constructor_fn(seeded_counter);

        family.with(&("route", "checkout".to_owned()), |counter| {
            counter.incr();
            assert_eq!(counter.get(), 42);
        });
    }
}
