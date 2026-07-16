//! A panic / early-exit through a measured expression is an *aborted* outcome:
//! ErrorCount must count it as an error, Elapsed must still record, and
//! InFlight must be balanced back. This is what the owned-recorder model
//! guarantees via the recorder's `Drop`.

use metered_semantic::{measure, Elapsed, ErrorCount, HitCount, InFlight};
use std::panic::{catch_unwind, AssertUnwindSafe};

#[test]
fn panic_counts_as_error_and_balances_inflight() {
    let hits: HitCount = HitCount::default();
    let errors: ErrorCount = ErrorCount::default();
    let inflight: InFlight = InFlight::default();
    let elapsed: Elapsed = Elapsed::default();

    let outcome = catch_unwind(AssertUnwindSafe(|| {
        measure!(
            &hits,
            measure!(
                &errors,
                measure!(
                    &inflight,
                    measure!(&elapsed, {
                        // While inside the body the gauge is held.
                        assert_eq!(inflight.get(), 1, "gauge incremented on entry");
                        panic!("boom");
                        #[allow(unreachable_code)]
                        Ok::<(), ()>(())
                    })
                )
            )
        )
    }));

    assert!(outcome.is_err(), "the panic should propagate out");
    assert_eq!(hits.get(), 1, "the hit is counted on entry");
    assert_eq!(errors.get(), 1, "a panic must count as an error");
    assert_eq!(inflight.get(), 0, "the in-flight gauge must be balanced");
    assert_eq!(
        elapsed.snapshot().count,
        1,
        "the elapsed time of the aborted call is still recorded"
    );
}

#[test]
fn normal_err_counts_as_error_ok_does_not() {
    let errors: ErrorCount = ErrorCount::default();

    let _ = measure!(&errors, { Ok::<(), ()>(()) });
    assert_eq!(errors.get(), 0, "Ok must not count as an error");

    let _ = measure!(&errors, { Err::<(), ()>(()) });
    assert_eq!(errors.get(), 1, "Err must count as an error");
}
