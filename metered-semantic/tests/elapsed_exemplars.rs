//! An `Elapsed` measured via `measure!` records into a cumulative bucket histogram
//! and attaches an exemplar (minted by a pluggable, tracing-agnostic source) to
//! the landing bucket -- which then rides the OpenMetrics output.

use metered::bucket_histogram::{Exemplar, ExemplarSource};
use metered::{Buckets, MetricTree};
use metered_om::OpenMetricsEncoder;
use metered_semantic::{measure, Elapsed, ElapsedConfig};

/// A stand-in for a consumer's source that reads the active trace. Here it
/// always mints the same labels; the observed value is filled in by `Elapsed`.
#[derive(Clone, Default)]
struct FixedTrace;

impl ExemplarSource for FixedTrace {
    fn exemplar(&self) -> Option<Exemplar> {
        Some(Exemplar {
            labels: vec![("trace_id".into(), "abc123".into())],
            value: 0.0,
            timestamp_seconds: None,
        })
    }
}

#[derive(Clone)]
struct ConfiguredTrace {
    trace_id: &'static str,
}

impl ExemplarSource for ConfiguredTrace {
    fn exemplar(&self) -> Option<Exemplar> {
        Some(Exemplar {
            labels: vec![("trace_id".into(), self.trace_id.into())],
            value: 0.0,
            timestamp_seconds: Some(42.0),
        })
    }
}

#[test]
fn measured_elapsed_records_with_exemplar_and_encodes() {
    let elapsed: Elapsed<FixedTrace> = Elapsed::default();

    let out = measure!(&elapsed, {
        std::thread::sleep(std::time::Duration::from_millis(1));
        7
    });
    assert_eq!(out, 7, "measure! returns the inner value");

    let snapshot = elapsed.snapshot();
    assert_eq!(snapshot.count, 1);
    let exemplar = snapshot
        .buckets
        .iter()
        .find_map(|b| b.exemplar.as_ref())
        .expect("an exemplar should be attached to the landing bucket");
    assert_eq!(exemplar.labels[0].1, "abc123");
    assert!(
        exemplar.value > 0.0,
        "exemplar value is the observed duration"
    );

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        elapsed
            .encode("op_duration_seconds", &[], &mut enc)
            .unwrap();
        enc.finish().unwrap();
    }
    assert!(buf.contains("# TYPE op_duration_seconds histogram"));
    assert!(buf.contains("op_duration_seconds_count 1"));
    assert!(buf.contains("# {trace_id=\"abc123\"}"));
}

#[test]
fn default_elapsed_has_no_exemplars() {
    let elapsed: Elapsed = Elapsed::default();
    measure!(&elapsed, {});
    let snapshot = elapsed.snapshot();
    assert_eq!(snapshot.count, 1);
    assert!(snapshot.buckets.iter().all(|b| b.exemplar.is_none()));
}

#[test]
fn elapsed_config_sets_buckets_and_exemplar_source() {
    let elapsed = Elapsed::with_config(ElapsedConfig {
        buckets: Buckets::custom([0.001]),
        exemplar_source: ConfiguredTrace {
            trace_id: "from-config",
        },
    });

    measure!(&elapsed, {});

    let snapshot = elapsed.snapshot();
    assert_eq!(snapshot.buckets.len(), 2, "configured finite bucket + +Inf");
    let exemplar = snapshot
        .buckets
        .iter()
        .find_map(|bucket| bucket.exemplar.as_ref())
        .expect("configured source should attach an exemplar");
    assert_eq!(exemplar.labels[0].1, "from-config");
    assert_eq!(exemplar.timestamp_seconds, Some(42.0));
}

#[test]
fn elapsed_debug_reports_sum_and_count() {
    let elapsed: Elapsed = Elapsed::default();
    measure!(&elapsed, {});

    let debug = format!("{:?}", elapsed);
    assert!(debug.contains("Elapsed"));
    assert!(debug.contains("sum"));
    assert!(debug.contains("count"));
}
