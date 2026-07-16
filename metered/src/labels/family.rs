//! Labeled metric families: one metric per label set, for dynamic label
//! dimensions.
//!
//! A [`Family`] maps a label set to a metric, creating metrics on first use --
//! the analogue of a Prometheus metric family. Use it when a label dimension is
//! dynamic (e.g. one counter per `method`, per `route`, per `status`). It pairs
//! with the natural-structure model (`#[metered]` registries / newtypes): use a
//! newtype for fixed metrics and a `Family` for dynamic label sets.
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

/// A set of label name/value pairs identifying one series within a [`Family`].
///
/// Implemented for `()` (no labels) and `Vec<(String, String)>` (fully dynamic).
/// Implement it for your own key type to get typed, validated label sets.
pub trait LabelSet {
    /// Appends this set's `(name, value)` pairs to `out`.
    fn encode_labels(&self, out: &mut Vec<(String, String)>);

    /// Appends label names known from the type alone.
    ///
    /// Fully dynamic label sets such as `Vec<(String, String)>` cannot know
    /// their shape statically; use [`Family::with_label_names`] for those.
    fn label_names(_out: &mut Vec<String>) {}
}

impl LabelSet for () {
    fn encode_labels(&self, _out: &mut Vec<(String, String)>) {}
}

impl LabelSet for Vec<(String, String)> {
    fn encode_labels(&self, out: &mut Vec<(String, String)>) {
        out.extend(self.iter().cloned());
    }
}

impl LabelSet for (&'static str, String) {
    fn encode_labels(&self, out: &mut Vec<(String, String)>) {
        out.push((self.0.to_owned(), self.1.clone()));
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
pub struct Family<L, M, C = fn() -> M> {
    metrics: RwLock<HashMap<L, M>>,
    constructor: C,
    label_names: Vec<String>,
}

impl<L: LabelSet + Clone + Hash + Eq, M: Default> Default for Family<L, M> {
    fn default() -> Self {
        let mut label_names = Vec::new();
        L::label_names(&mut label_names);
        Family {
            metrics: RwLock::new(HashMap::new()),
            constructor: M::default,
            label_names,
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
        }
    }
}

impl<L, M, C> MetricTree for Family<L, M, C>
where
    L: LabelSet + Clone + Hash + Eq,
    M: MetricTree,
    C: MetricConstructor<M>,
{
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        let all = with_labels(
            labels,
            self.label_names.iter().map(|name| (name.as_str(), "")),
        );

        if let Some(metric) = self.metrics.read().values().next() {
            metric.describe(name, &all, schema);
        } else {
            let metric = self.constructor.new_metric();
            metric.describe(name, &all, schema);
        }
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let map = self.metrics.read();
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
            metric.collect(name, &all, values);
        }
    }

    fn housekeep(&self) {
        for metric in self.metrics.read().values() {
            metric.housekeep();
        }
    }

    fn needs_housekeep(&self) -> bool {
        self.metrics
            .read()
            .values()
            .any(MetricTree::needs_housekeep)
    }
}

fn normalized_label_names(label_names: impl IntoIterator<Item = impl Into<String>>) -> Vec<String> {
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
    fn remove_drops_a_series() {
        let family: Family<(&'static str, String), std::sync::atomic::AtomicU64> =
            Family::default();
        family.with(&("conn", "a".to_owned()), |c| c.incr());
        assert_eq!(family.len(), 1);
        assert!(family.remove(&("conn", "a".to_owned())));
        assert!(family.is_empty());
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
