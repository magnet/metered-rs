use metered_semantic::metric::{Measure, Recorder};
use metered_semantic::{measure, ErrorCount, InFlight, NoneCount};

#[test]
fn none_count_records_only_none_outcomes() {
    let none = NoneCount::default();

    measure!(&none, Option::<u8>::None);
    measure!(&none, Some(1u8));
    let _ = measure!(&none, Result::<Option<u8>, &str>::Ok(None));
    let _ = measure!(&none, Result::<Option<u8>, &str>::Ok(Some(1)));
    let _ = measure!(&none, Result::<Option<u8>, &str>::Err("boom"));

    assert_eq!(none.get(), 2);
}

#[test]
fn error_count_records_err_and_abort_but_not_ok() {
    let errors = ErrorCount::default();

    let _ = measure!(&errors, Result::<(), &str>::Ok(()));
    let _ = measure!(&errors, Result::<(), &str>::Err("boom"));

    {
        let _aborted = errors.enter();
    }

    assert_eq!(errors.get(), 2);
}

#[test]
fn inflight_balances_complete_and_drop_paths() {
    let inflight = InFlight::default();

    {
        let mut recorder = inflight.enter();
        assert_eq!(inflight.get(), 1);
        recorder.complete(&());
        assert_eq!(inflight.get(), 0);
    }
    assert_eq!(inflight.get(), 0);

    {
        let _aborted = inflight.enter();
        assert_eq!(inflight.get(), 1);
    }
    assert_eq!(inflight.get(), 0);
}
