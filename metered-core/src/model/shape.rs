//! Adaptors that control the *shape* of a metric tree's names independently of
//! the code that produces it.
//!
//! A series' name is built from the path through the metric tree: a registry
//! prefix, the registered name, then a segment per nesting level (a struct
//! field, a `MetricTreeView` entry). That makes the wire name a function of code
//! structure, so renaming a field or extracting a sub-struct silently renames
//! metrics and breaks dashboards.
//!
//! [`Renamed`] and [`Flatten`] decouple the two: wrap a [`MetricTree`] to change
//! the segment it contributes ([`Renamed`]) or to drop its segment entirely so
//! its children sit at the parent level ([`Flatten`]). They are the runtime
//! equivalent of the `#[metric(rename = "...")]` and `#[metric(flatten)]` field
//! attributes on `#[derive(MetricTree)]`.
//!
//! Both transform the name uniformly in `describe` and `collect` (and therefore
//! in the default `encode`), so the schema and the values stay in lockstep.
//!
//! ```
//! use metered::{Counter, MetricTree};
//! use metered::shape::{Flatten, Renamed};
//! use metered_om::OpenMetricsEncoder;
//! use std::sync::atomic::AtomicU64;
//!
//! let hits = AtomicU64::new(0);
//! hits.incr();
//!
//! let mut buf = String::new();
//! {
//!     let mut enc = OpenMetricsEncoder::new(&mut buf);
//!     // Emit the counter under `requests` rather than the parent name.
//!     Renamed::new("requests", &hits).encode("api", &[], &mut enc).unwrap();
//!     enc.finish().unwrap();
//! }
//! assert!(buf.contains("api_requests_total 1"));
//! ```

use crate::metric_tree::{join_name, MetricTree};
use crate::schema::MetricSchema;
use crate::values::MetricValues;

/// A metric tree emitted under an explicit name segment.
///
/// Contributes `segment` to the path instead of whatever the surrounding code
/// would (a struct field name, a view entry name). Use it to keep a stable wire
/// name after renaming the Rust item.
pub struct Renamed<'a, M: ?Sized> {
    segment: &'a str,
    inner: &'a M,
}

impl<'a, M: ?Sized + MetricTree> Renamed<'a, M> {
    /// Emits `inner` under `segment` (joined onto the inherited name).
    pub fn new(segment: &'a str, inner: &'a M) -> Self {
        Renamed { segment, inner }
    }
}

impl<M: ?Sized + MetricTree> MetricTree for Renamed<'_, M> {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.inner
            .describe(&join_name(name, self.segment), labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.inner
            .collect(&join_name(name, self.segment), labels, values);
    }

    fn housekeep(&self) {
        self.inner.housekeep();
    }

    fn needs_housekeep(&self) -> bool {
        self.inner.needs_housekeep()
    }
}

/// A metric tree flattened into its parent: it contributes no name segment, so
/// its children are emitted directly under the inherited name.
///
/// Use it to extract a group of metrics into a sub-struct (or sub-view) for code
/// organization without changing any emitted names.
pub struct Flatten<'a, M: ?Sized> {
    inner: &'a M,
}

impl<'a, M: ?Sized + MetricTree> Flatten<'a, M> {
    /// Emits `inner` under the inherited name, with no extra segment.
    pub fn new(inner: &'a M) -> Self {
        Flatten { inner }
    }
}

impl<M: ?Sized + MetricTree> MetricTree for Flatten<'_, M> {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.inner.describe(name, labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.inner.collect(name, labels, values);
    }

    fn housekeep(&self) {
        self.inner.housekeep();
    }

    fn needs_housekeep(&self) -> bool {
        self.inner.needs_housekeep()
    }
}

