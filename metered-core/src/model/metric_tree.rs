//! The [`MetricTree`] trait: the contract that ties the
//! [`schema`](crate::schema) and [`values`](crate::values) together, plus the
//! leaf [`Metric`] trait and the [`MetricType`] classification both share.
//!
//! A `MetricTree` can describe its schema and collect its values; the default
//! `encode` is exactly "describe + collect + write to a sink", so a tree can
//! never advertise one shape and emit another. The concrete sink (OpenMetrics
//! text, ...) lives in a separate crate; see [`MetricSink`].

use crate::meta::{Help, Unit};
use crate::schema::MetricSchema;
use crate::sink::{MetricSink, SinkError};
use crate::values::MetricValues;
use std::sync::Arc;

/// The OpenMetrics metric type written on a `# TYPE` line.
///
/// `#[non_exhaustive]`: metric types added by a future OpenMetrics revision
/// can be introduced without a breaking change, so external `match`es must
/// include a wildcard arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MetricType {
    /// A monotonically increasing counter (sample suffix `_total`).
    Counter,
    /// A value that can go up and down.
    Gauge,
    /// A cumulative histogram (`_bucket` / `_sum` / `_count`).
    Histogram,
    /// A summary (pre-computed quantiles + `_sum` / `_count`).
    Summary,
    /// Static key/value metadata, always value `1`.
    Info,
    /// A set of mutually-exclusive boolean states.
    StateSet,
    /// A metric of unknown semantics (e.g. a passthrough foreign metric).
    /// Renders a plain sample with no suffix.
    Unknown,
    /// A histogram of current (non-cumulative) values: cumulative `le` buckets
    /// whose counts may decrease, plus `_gcount` and `_gsum`.
    GaugeHistogram,
}

impl MetricType {
    /// The OpenMetrics `# TYPE` token for this metric type.
    pub fn as_str(self) -> &'static str {
        match self {
            MetricType::Counter => "counter",
            MetricType::Gauge => "gauge",
            MetricType::Histogram => "histogram",
            MetricType::Summary => "summary",
            MetricType::Info => "info",
            MetricType::StateSet => "stateset",
            MetricType::Unknown => "unknown",
            MetricType::GaugeHistogram => "gaugehistogram",
        }
    }
}

/// A leaf metric.
///
/// `Metric` couples the metric family type used for schema generation with the
/// sample collection for the same leaf. Composite metric trees should implement
/// [`MetricTree`] directly.
pub trait Metric {
    /// The OpenMetrics type for this family.
    fn metric_type(&self) -> MetricType;

    /// Collects the current samples for this metric.
    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues);

    /// Describes this metric family. Override when labels are intrinsic to the
    /// metric itself (for example `Info` and `StateSet`).
    fn describe_metric(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        schema.add_family(name, self.metric_type(), labels);
    }

    /// Performs off-hot-path structural upkeep (e.g. rescaling a dynamic
    /// exponential histogram). The default is a no-op; only metrics with a
    /// dynamically-sized backing structure override it.
    ///
    /// Takes `&self` (upkeep happens through interior mutability / RCU) and is
    /// safe to call concurrently with observation. It should be driven by at
    /// most one thread at a time -- the [`Registry`](crate::Registry) does this
    /// on scrape by default; see [`Metric::needs_housekeep`].
    fn housekeep(&self) {}

    /// Whether [`housekeep`](Metric::housekeep) has work to do.
    ///
    /// Defaults to `false`, which for static dispatch inlines away the upkeep
    /// call entirely; only dynamic metrics override it. The
    /// [`Registry`](crate::Registry) uses it to skip whole subtrees cheaply at
    /// its `dyn` boundary.
    fn needs_housekeep(&self) -> bool {
        false
    }
}

/// Joins a metric-name prefix with a segment using `_`. An empty side
/// contributes nothing -- in particular an empty segment (e.g.
/// `Renamed::new("", ...)` or a flattened mount) yields the prefix unchanged,
/// never a trailing `_`. Used by generated registry code to build hierarchical
/// metric names.
pub fn join_name(prefix: &str, segment: &str) -> String {
    if prefix.is_empty() {
        segment.to_string()
    } else if segment.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}_{segment}")
    }
}

/// A metric tree: either one leaf metric or a composite tree of metrics.
///
/// `MetricTree` is what can be registered under a [`Registry`](crate::Registry)
/// prefix. Leaf metrics implement it through the blanket [`Metric`]
/// implementation; generated registries and view structs implement it by
/// recursively describing/collecting their fields.
pub trait MetricTree {
    /// Encodes this tree under `name` with inherited constant `labels` into
    /// `sink`.
    ///
    /// The default is the single rendering path: describe the schema, collect
    /// the values, and write them to the sink together, so a tree can never
    /// advertise one shape and emit another. The sink chooses the wire format.
    ///
    /// This is also the scrape boundary for a directly-rendered tree (e.g.
    /// `OpenMetricsExt::encode_to_string`), so it drives off-hot-path upkeep
    /// first when any metric asks for it -- the same self-maintenance
    /// [`Registry::values`](crate::Registry::values) performs. Without it, a
    /// [`DynamicExponentialHistogram`](crate::DynamicExponentialHistogram) on
    /// this path would never rescale and would freeze its sampled exemplar after
    /// the first adoption. A `metered_om::SnapshotCache` drives `housekeep`
    /// itself and renders through `describe`/`collect`, not `encode`, so it
    /// never double-maintains.
    fn encode(
        &self,
        name: &str,
        labels: &[(&str, &str)],
        sink: &mut dyn MetricSink,
    ) -> Result<(), SinkError> {
        if self.needs_housekeep() {
            self.housekeep();
        }
        let mut schema = MetricSchema::new();
        self.describe(name, labels, &mut schema);
        let mut values = MetricValues::new();
        self.collect(name, labels, &mut values);
        sink.encode_document(&schema, &values)
    }

    /// Adds this tree's OpenMetrics families to `schema`.
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema);

    /// Collects this tree's current values.
    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues);

    /// Performs off-hot-path structural upkeep across this tree (e.g. rescaling
    /// dynamic exponential histograms). Default no-op; composite trees forward
    /// it to their children. See [`Metric::housekeep`].
    fn housekeep(&self) {}

    /// Whether any metric in this tree currently needs
    /// [`housekeep`](MetricTree::housekeep). Default `false`; composites OR
    /// their children so a clean subtree can be skipped without descending.
    fn needs_housekeep(&self) -> bool {
        false
    }
}

/// Static metadata associated with a self-contained metric tree.
///
/// Derive-generated trees use this to expose container-level
/// `#[metric(help = "...", unit = "...")]` metadata without requiring an
/// instance.
pub trait MetricTreeMeta {
    /// HELP text associated with the root metric tree, if present.
    fn help() -> Option<Help> {
        None
    }

    /// UNIT metadata associated with the root metric tree, if present.
    fn unit() -> Option<Unit> {
        None
    }
}

impl<T: Metric> MetricTree for T {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.describe_metric(name, labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.collect_metric(name, labels, values);
    }

    fn housekeep(&self) {
        Metric::housekeep(self);
    }

    fn needs_housekeep(&self) -> bool {
        Metric::needs_housekeep(self)
    }
}

/// An optional sub-tree: `Some` forwards to the inner tree unchanged, `None` is
/// a complete no-op (contributes no families and no samples).
///
/// This lets a composite tree gate a group of metrics on construction -- a
/// `None` field simply does not exist on the scrape -- while still being
/// `#[derive(MetricTree)]`-able (typically through `#[metric(flatten)]`).
impl<T: MetricTree> MetricTree for Option<T> {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        if let Some(inner) = self {
            inner.describe(name, labels, schema);
        }
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        if let Some(inner) = self {
            inner.collect(name, labels, values);
        }
    }

    fn housekeep(&self) {
        if let Some(inner) = self {
            inner.housekeep();
        }
    }

    fn needs_housekeep(&self) -> bool {
        self.as_ref().is_some_and(MetricTree::needs_housekeep)
    }
}

/// A thread-local visited-pointer set for the indirection-forwarding
/// [`MetricTree`] impls.
///
/// A mounted tree can, in safe code, transitively reach itself: a shared handle
/// (`Arc<T>`, and any `Arc<dyn MetricTree>` reached through it) can be closed
/// into a loop -- e.g. `Arc<RwLock<Option<Arc<dyn MetricTree>>>>`. The
/// OpenMetrics text encoder is iterative, but the describe / collect / housekeep
/// / needs_housekeep walks recurse over the mount graph, so a cycle overflows the
/// stack (observed as a production crash). A cycle can only be formed through a
/// shared handle, so tracking the `Arc` data pointers on the **active descent
/// path** catches exactly the cycles: revisiting a pointer already on the path
/// is a cycle and stops; a deep-but-acyclic graph (or the same handle mounted
/// twice as siblings, a DAG) walks in full. The set is a thread-local `Vec`
/// used as a stack -- push on enter, pop on drop -- so the check is a short
/// linear scan over the current path depth, with no depth heuristic to
/// misclassify legitimate deep trees.
mod cycle_guard {
    use std::cell::RefCell;

    thread_local! {
        /// The `Arc` data pointers on the current walk's descent path.
        static PATH: RefCell<Vec<*const ()>> = const { RefCell::new(Vec::new()) };
    }

    /// RAII marker for one shared-handle hop; pops its pointer on drop.
    pub(super) struct Guard;

    impl Guard {
        /// Enters the hop through the shared handle at `ptr`, or returns
        /// `None` when `ptr` is already on the active descent path -- i.e. the
        /// walk has come back around to a handle it is currently inside, a
        /// true cycle.
        pub(super) fn enter(ptr: *const ()) -> Option<Guard> {
            PATH.with(|path| {
                let mut path = path.borrow_mut();
                if path.contains(&ptr) {
                    return None;
                }
                path.push(ptr);
                Some(Guard)
            })
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            PATH.with(|path| {
                path.borrow_mut().pop();
            });
        }
    }
}

/// Signals a truncated descent. Never panics on the scrape path: a cyclic mount
/// graph must not abort exposition, so in debug builds this only writes a
/// diagnostic (it is a no-op cold call in release). We do not use `debug_assert!`
/// here precisely because the guard must let a cyclic scrape terminate with `Ok`
/// rather than panic mid-render.
#[cold]
fn metric_tree_cycle_detected(_method: &str) {
    #[cfg(debug_assertions)]
    eprintln!(
        "metered: MetricTree::{_method} revisited a shared handle already on \
         the active walk path; the cyclic mount graph was truncated at the \
         revisit to avoid a stack overflow"
    );
}

/// A shared sub-tree: forwards transparently to the inner tree.
///
/// Lets an `Arc<T>` handle be a `#[derive(MetricTree)]` field (typically
/// `#[metric(flatten)]`) without a hand-written forwarding impl -- the common
/// shape for a cheaply-`Clone` metrics handle wrapping a derived `*State`. The
/// `?Sized` bound also admits `Arc<dyn MetricTree>`. Mirrors the [`Option<T>`]
/// impl above.
///
/// Every method is bounded by the internal cycle guard: `Arc` is the shared handle
/// through which a mount graph can, in safe code, be closed into a cycle, so
/// tracking the visited handles here stops such a cycle from overflowing the
/// stack during a describe / collect / housekeep / needs_housekeep walk --
/// while a deep-but-acyclic graph walks in full.
impl<T: MetricTree + ?Sized> MetricTree for Arc<T> {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        let Some(_guard) = cycle_guard::Guard::enter(Arc::as_ptr(self) as *const ()) else {
            metric_tree_cycle_detected("describe");
            return;
        };
        (**self).describe(name, labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let Some(_guard) = cycle_guard::Guard::enter(Arc::as_ptr(self) as *const ()) else {
            metric_tree_cycle_detected("collect");
            return;
        };
        (**self).collect(name, labels, values);
    }

    fn housekeep(&self) {
        let Some(_guard) = cycle_guard::Guard::enter(Arc::as_ptr(self) as *const ()) else {
            metric_tree_cycle_detected("housekeep");
            return;
        };
        (**self).housekeep();
    }

    fn needs_housekeep(&self) -> bool {
        let Some(_guard) = cycle_guard::Guard::enter(Arc::as_ptr(self) as *const ()) else {
            metric_tree_cycle_detected("needs_housekeep");
            return false;
        };
        (**self).needs_housekeep()
    }
}

/// Root-level access to a self-contained metric tree's schema and values.
///
/// A root tree that carries its own name and labels -- e.g. a
/// `#[derive(MetricTree)]` struct with `#[metric(prefix = "...", label(...))]`
/// -- exposes its [`schema`](MetricTreeExt::schema) and
/// [`values`](MetricTreeExt::values) at the document root, ready to hand to a
/// sink. To render straight to an OpenMetrics document string, bring
/// `metered_om::OpenMetricsExt` into scope for `encode_to_string`:
///
/// ```
/// use metered::{Counter, MetricTree};
/// use metered_om::OpenMetricsExt;
/// use std::sync::atomic::AtomicU64;
///
/// #[derive(Default, MetricTree)]
/// #[metrics(prefix = "app", label(service = "app"))]
/// struct AppMetrics {
///     #[metrics(counter)]
///     requests: AtomicU64,
/// }
///
/// let metrics = AppMetrics::default();
/// metrics.requests.incr();
/// let text = metrics.encode_to_string().unwrap();
/// assert!(text.contains("# TYPE app_requests counter"));
/// assert!(text.contains("app_requests_total{service=\"app\"} 1"));
/// ```
///
/// Use a [`Registry`](crate::Registry) or [`MetricTreeView`](crate::MetricTreeView)
/// instead when composing several trees or applying a shared prefix/labels at the
/// exposition site.
pub trait MetricTreeExt: MetricTree {
    /// The schema this tree describes at the document root.
    fn schema(&self) -> MetricSchema {
        let mut schema = MetricSchema::new();
        self.describe("", &[], &mut schema);
        schema
    }

    /// The current values this tree collects at the document root.
    fn values(&self) -> MetricValues {
        let mut values = MetricValues::new();
        self.collect("", &[], &mut values);
        values
    }
}

impl<T: MetricTree + ?Sized> MetricTreeExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bucket_histogram::BucketHistogram;
    use crate::{InfoMetric, StateSet};
    use std::sync::atomic::{AtomicI64, AtomicU64};

    #[test]
    fn describe_metric_impls_cover_core_types() {
        let mut schema = MetricSchema::new();
        AtomicU64::new(0).describe("requests", &[], &mut schema);
        AtomicI64::new(0).describe("depth", &[], &mut schema);
        InfoMetric::new([("version", "1")]).describe("build", &[], &mut schema);
        let states = StateSet::new(["starting", "running"]);
        states.describe("state", &[], &mut schema);
        BucketHistogram::default().describe("latency", &[], &mut schema);

        assert_eq!(
            schema.family("requests").unwrap().metric_type,
            MetricType::Counter
        );
        assert_eq!(
            schema.family("depth").unwrap().metric_type,
            MetricType::Gauge
        );
        assert_eq!(
            schema.family("build").unwrap().metric_type,
            MetricType::Info
        );
        assert_eq!(
            schema.family("state").unwrap().metric_type,
            MetricType::StateSet
        );
        assert_eq!(
            schema.family("latency").unwrap().metric_type,
            MetricType::Histogram
        );
    }

    #[test]
    fn option_some_forwards_like_inner_and_none_is_a_noop() {
        use crate::Counter;

        // `Some(metric)` describes and collects identically to the bare metric.
        let bare = AtomicU64::new(0);
        Counter::incr(&bare);
        let mut bare_schema = MetricSchema::new();
        bare.describe("requests", &[], &mut bare_schema);
        let mut bare_values = MetricValues::new();
        bare.collect("requests", &[], &mut bare_values);

        let some = Some({
            let metric = AtomicU64::new(0);
            Counter::incr(&metric);
            metric
        });
        let mut some_schema = MetricSchema::new();
        some.describe("requests", &[], &mut some_schema);
        let mut some_values = MetricValues::new();
        some.collect("requests", &[], &mut some_values);

        let family_shape = |schema: &MetricSchema| {
            schema
                .families()
                .iter()
                .map(|family| (family.name.clone(), family.metric_type))
                .collect::<Vec<_>>()
        };
        let sample_shape = |values: &MetricValues| {
            values
                .samples()
                .iter()
                .map(|sample| (sample.name.clone(), sample.value))
                .collect::<Vec<_>>()
        };
        assert_eq!(family_shape(&some_schema), family_shape(&bare_schema));
        assert_eq!(sample_shape(&some_values), sample_shape(&bare_values));
        assert!(!some_values.samples().is_empty());

        // `None` contributes zero families and zero samples.
        let none: Option<AtomicU64> = None;
        let mut none_schema = MetricSchema::new();
        none.describe("requests", &[], &mut none_schema);
        let mut none_values = MetricValues::new();
        none.collect("requests", &[], &mut none_values);
        assert!(none_schema.families().is_empty());
        assert!(none_values.samples().is_empty());
        assert!(!none.needs_housekeep());
        none.housekeep();
    }

    #[test]
    fn arc_forwards_like_the_inner_tree() {
        use crate::Counter;
        use std::sync::Arc;

        // A bare metric and the same metric behind an `Arc` describe and
        // collect identically -- so an `Arc<T>` handle can flatten transparently
        // into a derived tree.
        let bare = AtomicU64::new(0);
        Counter::incr(&bare);
        let mut bare_schema = MetricSchema::new();
        bare.describe("requests", &[], &mut bare_schema);
        let mut bare_values = MetricValues::new();
        bare.collect("requests", &[], &mut bare_values);

        let shared = Arc::new({
            let metric = AtomicU64::new(0);
            Counter::incr(&metric);
            metric
        });
        let mut shared_schema = MetricSchema::new();
        shared.describe("requests", &[], &mut shared_schema);
        let mut shared_values = MetricValues::new();
        shared.collect("requests", &[], &mut shared_values);

        let family_names = |schema: &MetricSchema| {
            schema
                .families()
                .iter()
                .map(|family| (family.name.clone(), family.metric_type))
                .collect::<Vec<_>>()
        };
        let sample_values = |values: &MetricValues| {
            values
                .samples()
                .iter()
                .map(|sample| (sample.name.clone(), sample.value))
                .collect::<Vec<_>>()
        };
        assert_eq!(family_names(&shared_schema), family_names(&bare_schema));
        assert_eq!(sample_values(&shared_values), sample_values(&bare_values));
        assert!(!shared.needs_housekeep());
        shared.housekeep();
    }
}
