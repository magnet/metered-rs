//! Bounded label-value interning: cardinality discipline for high-churn labels.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// The overflow label value emitted once a [`BoundedValues`] cap is reached,
/// matching the OpenTelemetry convention for unrecognized values.
pub const OTHER: &str = "_OTHER";

/// A bounded set of interned label values.
///
/// High-churn label sources (request paths, RPC method names from untrusted
/// peers) can explode a metric family's cardinality. `BoundedValues` interns
/// up to `cap` distinct values; anything beyond resolves to [`OTHER`]
/// (`"_OTHER"`), following the OpenTelemetry semantic-convention rule for
/// unrecognized `rpc.method` values. Interning also dedupes allocations: a
/// hot label value is one `Arc<str>` shared by every series touch.
///
/// Prefer [`with_known`](BoundedValues::with_known) when the legitimate value
/// set is known up front (e.g. the methods of a gRPC service): recognized
/// values never overflow, and the cap only guards the unexpected.
#[derive(Debug)]
pub struct BoundedValues {
    cap: usize,
    other: Arc<str>,
    values: RwLock<HashMap<Box<str>, Arc<str>>>,
}

impl BoundedValues {
    /// Creates an interner that holds at most `cap` distinct values.
    pub fn new(cap: usize) -> Self {
        BoundedValues {
            cap,
            other: Arc::from(OTHER),
            values: RwLock::new(HashMap::new()),
        }
    }

    /// Creates an interner pre-seeded with `known` values; the seeds count
    /// toward `cap` but are guaranteed present (the cap is raised to fit them).
    pub fn with_known<I, S>(cap: usize, known: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut map = HashMap::new();
        for value in known {
            let value = value.as_ref();
            map.insert(Box::from(value), Arc::from(value));
        }
        BoundedValues {
            cap: cap.max(map.len()),
            other: Arc::from(OTHER),
            values: RwLock::new(map),
        }
    }

    /// Resolves `value` to its interned form, or [`OTHER`] once the cap is hit.
    pub fn bound(&self, value: &str) -> Arc<str> {
        if let Some(interned) = self.values.read().get(value) {
            return interned.clone();
        }
        let mut values = self.values.write();
        if let Some(interned) = values.get(value) {
            return interned.clone();
        }
        if values.len() >= self.cap {
            return self.other.clone();
        }
        let interned: Arc<str> = Arc::from(value);
        values.insert(Box::from(value), interned.clone());
        interned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values_intern_and_dedupe() {
        let values = BoundedValues::new(2);
        let a1 = values.bound("alpha");
        let a2 = values.bound("alpha");
        assert_eq!(&*a1, "alpha");
        // Same Arc, not a new allocation.
        assert!(std::sync::Arc::ptr_eq(&a1, &a2));
    }

    #[test]
    fn overflow_collapses_to_other() {
        let values = BoundedValues::new(2);
        values.bound("a");
        values.bound("b");
        assert_eq!(&*values.bound("c"), "_OTHER");
        // Known values keep resolving after the cap is hit.
        assert_eq!(&*values.bound("a"), "a");
    }

    #[test]
    fn preregistered_values_never_overflow() {
        let values = BoundedValues::with_known(1, ["GetOrder"]);
        assert_eq!(&*values.bound("GetOrder"), "GetOrder");
        assert_eq!(&*values.bound("anything-else"), "_OTHER");
    }
}
