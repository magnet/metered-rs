//! End-to-end: a `#[metered]` service renders its whole registry -- counters,
//! gauge, and an `#[error_count]` breakdown -- natively to OpenMetrics text.

use metered::MetricTree;
use metered_om::OpenMetricsEncoder;
use metered_semantic::{metered, ErrorCount, HitCount, InFlight};

#[metered_semantic::error_count(name = SvcErrors, visibility = pub)]
pub enum SvcError {
    Timeout,
    Backend,
}

#[derive(Default)]
pub struct Svc {
    metrics: SvcMetrics,
}

#[metered(registry = SvcMetrics)]
impl Svc {
    #[measure([HitCount, InFlight, ErrorCount, SvcErrors])]
    pub fn handle(&self, fail: Option<SvcError>) -> Result<(), SvcError> {
        match fail {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[test]
fn metered_registry_encodes_to_openmetrics() {
    let svc = Svc::default();
    let _ = svc.handle(None);
    let _ = svc.handle(Some(SvcError::Timeout));
    let _ = svc.handle(Some(SvcError::Timeout));

    let mut buf = String::new();
    {
        let mut enc = OpenMetricsEncoder::new(&mut buf);
        svc.metrics
            .encode("svc", &[("env", "test")], &mut enc)
            .unwrap();
        enc.finish().unwrap();
    }

    // Hierarchical names: <prefix>_<method>_<metric>.
    assert!(buf.contains("# TYPE svc_handle_hit_count counter"));
    assert!(buf.contains("svc_handle_hit_count_total{env=\"test\"} 3"));
    assert!(buf.contains("# TYPE svc_handle_in_flight gauge"));
    assert!(buf.contains("svc_handle_in_flight{env=\"test\"} 0"));
    assert!(buf.contains("svc_handle_error_count_total{env=\"test\"} 2"));

    // Breakdown: one counter family, error_kind label per variant.
    assert!(buf.contains("# TYPE svc_handle_svc_errors counter"));
    assert!(buf.contains("svc_handle_svc_errors_total{env=\"test\",error_kind=\"Timeout\"} 2"));
    assert!(buf.contains("svc_handle_svc_errors_total{env=\"test\",error_kind=\"Backend\"} 0"));

    assert!(buf.trim_end().ends_with("# EOF"));
}
