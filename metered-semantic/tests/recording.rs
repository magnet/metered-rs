#![cfg(feature = "recording")]

use metered::Registry;
use metered_om::OpenMetricsRegistryExt;
use metered_semantic::recording::Operation;

#[test]
fn operation_records_success_failure_in_flight_and_duration() {
    let op = Operation::default();

    let ok = op.record(|| Ok::<_, &'static str>(42));
    let err = op.record(|| Err::<u32, _>("boom"));

    assert_eq!(ok, Ok(42));
    assert_eq!(err, Err("boom"));
    assert_eq!(op.started.get(), 2);
    assert_eq!(op.completed.get(), 2);
    assert_eq!(op.failed.get(), 1);
    assert_eq!(op.in_flight.get(), 0);
    assert_eq!(op.duration.snapshot().count, 2);
}

#[test]
fn measure_macro_works_with_recording_operation() {
    let op = Operation::default();

    let value = metered_semantic::measure!(&op, { Ok::<_, &'static str>(7) });

    assert_eq!(value, Ok(7));
    assert_eq!(op.started.get(), 1);
    assert_eq!(op.completed.get(), 1);
    assert_eq!(op.failed.get(), 0);
}

#[test]
fn operation_records_panic_as_failure_and_balances_in_flight() {
    let op = Operation::default();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _recorder = metered_semantic::metric::Measure::enter(&op);
        panic!("boom");
    }));

    assert!(result.is_err());
    assert_eq!(op.started.get(), 1);
    assert_eq!(op.completed.get(), 0);
    assert_eq!(op.failed.get(), 1);
    assert_eq!(op.in_flight.get(), 0);
    assert_eq!(op.duration.snapshot().count, 1);
}

#[test]
fn operation_exports_as_metric_tree() {
    let op = Operation::default();
    let _ = op.record(|| Ok::<_, &'static str>(()));
    let _ = op.record(|| Err::<(), _>("boom"));

    let mut registry = Registry::with_prefix("app");
    registry.register(
        metered::entry::metric("refresh")
            .source(&op)
            .help("Refresh operation"),
    );

    let text = registry.encode_to_string().unwrap();
    assert!(text.contains("# TYPE app_refresh_started counter"));
    assert!(text.contains("app_refresh_started_total 2"));
    assert!(text.contains("# TYPE app_refresh_completed counter"));
    assert!(text.contains("app_refresh_completed_total 2"));
    assert!(text.contains("# TYPE app_refresh_failed counter"));
    assert!(text.contains("app_refresh_failed_total 1"));
    assert!(text.contains("# TYPE app_refresh_in_flight gauge"));
    assert!(text.contains("app_refresh_in_flight 0"));
    assert!(text.contains("# TYPE app_refresh_duration_seconds histogram"));
    assert!(text.contains("app_refresh_duration_seconds_count 2"));
}
