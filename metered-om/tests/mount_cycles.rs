//! A cyclic mount graph must terminate rather than overflow the stack -- and
//! only a *cycle* may be truncated.
//!
//! In safe code a mounted tree can transitively reach itself through a shared
//! `Arc` handle (e.g. `Arc<RwLock<Option<Arc<dyn MetricTree>>>>` closed into a
//! loop). The describe / collect / housekeep / needs_housekeep walks recurse
//! over the mount graph, so without a guard such a cycle overflows the stack
//! (observed as a production crash). The `Arc<T>` forwarding impl tracks the
//! shared handles on the active walk path, so a cyclic scrape terminates with
//! `Ok` at the revisit while deep acyclic chains and DAGs walk in full.

use metered::{MetricSchema, MetricTree, MetricValues};
use metered_om::OpenMetricsExt;
use std::sync::{Arc, RwLock};

/// A node that forwards its metric walk across a mutable link, so two nodes can
/// be closed into a genuine cycle. The link is `Arc<dyn MetricTree>`, so each hop
/// goes through the guarded `Arc<T>` forwarding impl.
type Link = Arc<RwLock<Option<Arc<dyn MetricTree + Send + Sync>>>>;

struct Node {
    next: Link,
}

impl Node {
    fn next(&self) -> Option<Arc<dyn MetricTree + Send + Sync>> {
        self.next.read().unwrap().clone()
    }
}

impl MetricTree for Node {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        if let Some(next) = self.next() {
            next.describe(name, labels, schema);
        }
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        if let Some(next) = self.next() {
            next.collect(name, labels, values);
        }
    }

    fn housekeep(&self) {
        if let Some(next) = self.next() {
            next.housekeep();
        }
    }

    fn needs_housekeep(&self) -> bool {
        self.next().is_some_and(|next| next.needs_housekeep())
    }
}

#[test]
fn cyclic_mount_graph_terminates_instead_of_overflowing() {
    let a: Arc<Node> = Arc::new(Node {
        next: Arc::new(RwLock::new(None)),
    });
    let b: Arc<Node> = Arc::new(Node {
        next: Arc::new(RwLock::new(None)),
    });
    // Close the loop: a -> b -> a.
    *a.next.write().unwrap() = Some(b.clone() as Arc<dyn MetricTree + Send + Sync>);
    *b.next.write().unwrap() = Some(a.clone() as Arc<dyn MetricTree + Send + Sync>);

    // encode_to_string runs describe + collect; both directions must terminate
    // (the depth guard truncates the cycle) and return Ok instead of overflowing.
    assert!(
        a.encode_to_string().is_ok(),
        "describe+collect from a terminates"
    );
    assert!(
        b.encode_to_string().is_ok(),
        "describe+collect from b terminates"
    );

    // The maintenance walks recurse over the same graph and must terminate too.
    assert!(
        !a.needs_housekeep(),
        "needs_housekeep terminates and finds no upkeep"
    );
    assert!(!b.needs_housekeep());
    a.housekeep();
    b.housekeep();

    // Break the reference cycle so the nodes are not leaked.
    *a.next.write().unwrap() = None;
    *b.next.write().unwrap() = None;
}

#[test]
fn deep_acyclic_chain_walks_in_full() {
    use metered::Counter;
    use std::sync::atomic::AtomicU64;

    // A counter behind a 512-deep chain of `Arc` hops: much deeper than any
    // depth heuristic would allow, but acyclic -- so the walk must reach the
    // leaf, not get truncated as a suspected cycle.
    let counter = AtomicU64::new(0);
    counter.incr();
    let mut tree: Arc<dyn MetricTree + Send + Sync> = Arc::new(counter);
    for _ in 0..512 {
        tree = Arc::new(tree) as Arc<dyn MetricTree + Send + Sync>;
    }

    let text = tree.encode_to_string().unwrap();
    // The unnamed counter leaf renders exactly one `_total 1` sample line.
    assert!(
        text.lines().any(|line| line == "_total 1"),
        "the leaf at depth 512 was reached:\n{text}"
    );
    let mut values = MetricValues::new();
    tree.collect("deep", &[], &mut values);
    assert_eq!(values.samples().len(), 1, "the deep leaf sample collected");
}

#[test]
fn shared_handle_mounted_twice_as_siblings_is_not_a_cycle() {
    use metered::Counter;
    use std::sync::atomic::AtomicU64;

    // The same Arc mounted under two sibling names is a DAG, not a cycle: the
    // walk leaves the handle before visiting it again, so both mounts emit.
    let counter = AtomicU64::new(0);
    counter.incr_by(3);
    let shared: Arc<AtomicU64> = Arc::new(counter);

    struct TwoMounts {
        shared: Arc<AtomicU64>,
    }

    impl MetricTree for TwoMounts {
        fn describe(&self, _: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
            self.shared.describe("first", labels, schema);
            self.shared.describe("second", labels, schema);
        }

        fn collect(&self, _: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
            self.shared.collect("first", labels, values);
            self.shared.collect("second", labels, values);
        }
    }

    let tree = TwoMounts { shared };
    let mut values = MetricValues::new();
    tree.collect("", &[], &mut values);
    let names: Vec<_> = values
        .samples()
        .iter()
        .map(|sample| sample.name.as_str())
        .collect();
    assert_eq!(
        names,
        vec!["first_total", "second_total"],
        "both sibling mounts of the shared handle emitted"
    );
}
