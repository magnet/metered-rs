//! With the `exemplar-context` feature, a `Elapsed<ThreadLocalExemplars>` picks up
//! the ambient exemplar a tracing layer set for the current scope.

#![cfg(feature = "exemplar-context")]

use metered::bucket_histogram::{with_exemplar, Exemplar, ThreadLocalExemplars};
use metered_semantic::{measure, Elapsed};

#[test]
fn timer_attaches_the_ambient_exemplar() {
    let elapsed: Elapsed<ThreadLocalExemplars> = Elapsed::default();

    let exemplar = Exemplar {
        labels: vec![("trace_id".to_owned(), "abc123".to_owned())],
        value: 0.0,
        timestamp_seconds: None,
    };

    with_exemplar(exemplar, || {
        measure!(&elapsed, {
            std::thread::sleep(std::time::Duration::from_millis(1));
        });
    });

    let snapshot = elapsed.snapshot();
    assert_eq!(snapshot.count, 1);
    let attached = snapshot
        .buckets
        .iter()
        .find_map(|b| b.exemplar.as_ref())
        .expect("the ambient exemplar should be attached to the landing bucket");
    assert_eq!(attached.labels[0].1, "abc123");
    assert!(attached.value > 0.0);

    // Outside the scope, no ambient exemplar is attached.
    let plain: Elapsed<ThreadLocalExemplars> = Elapsed::default();
    measure!(&plain, {});
    assert!(plain
        .snapshot()
        .buckets
        .iter()
        .all(|b| b.exemplar.is_none()));
}
