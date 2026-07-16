//! Renders the same pair of exponential histograms in both bucket profiles --
//! classic cumulative `le` and VictoriaMetrics non-cumulative `vmrange` -- so
//! the two expositions can be imported into a real backend and its
//! cross-series `histogram_quantile` compared against the printed ground
//! truth.
//!
//! The two series deliberately end up with different dynamic layouts:
//!
//! * `Fast`: a tight ~120us distribution (a fast method).
//! * `Slow`: a ~15ms distribution plus a single 2s outlier, the kind of
//!   first-call spike that stretches a [`DynamicExponentialHistogram`]'s
//!   layout and forces it onto a coarser schema than its siblings.
//!
//! Aggregating quantiles *across* series with mismatched classic `le` layouts
//! is unsound (bucket boundaries do not line up); the non-cumulative `vmrange`
//! form is built for exactly that. Run this, load each output into the target
//! system, and compare `histogram_quantile(0.5|0.9|0.99, ...)` over both
//! series against the ground-truth line.
//!
//! ```sh
//! cargo run -p metered-om --example vmrange_quantile_check
//! ```

use std::sync::Arc;

use metered::{DynamicExponentialHistogram, Family, LabelSet, MetricTree};
use metered_om::{HistogramProfile, MetricTreeSnapshot, SnapshotCache, SnapshotConfig};

#[derive(Clone, Debug, PartialEq, Eq, Hash, LabelSet)]
struct MethodLabels {
    rpc_method: String,
}

#[derive(Debug, Default, MetricTree)]
struct CallMetricsDemo {
    #[metric(rename = "call_duration_seconds", unit = "seconds")]
    duration: Family<MethodLabels, Arc<DynamicExponentialHistogram>>,
}

fn observe_all(histogram: &DynamicExponentialHistogram, values: &[f64]) {
    for value in values {
        histogram.observe(*value);
    }
}

fn main() {
    let tree = Arc::new(CallMetricsDemo::default());

    // Ground-truth samples.
    let fast: Vec<f64> = (0..1000).map(|i| 100e-6 + (i % 40) as f64 * 1e-6).collect();
    let mut slow: Vec<f64> = (0..200).map(|i| 12e-3 + (i % 30) as f64 * 0.4e-3).collect();
    slow.push(2.0); // outlier stretches the layout

    let series_fast = tree.duration.with(
        &MethodLabels {
            rpc_method: "Fast".into(),
        },
        Arc::clone,
    );
    let series_slow = tree.duration.with(
        &MethodLabels {
            rpc_method: "Slow".into(),
        },
        Arc::clone,
    );
    observe_all(&series_fast, &fast);
    observe_all(&series_slow, &slow);

    // Nearest-rank quantiles over the merged raw samples: the reference the
    // backend's bucket-based estimates should approximate.
    let mut truth: Vec<f64> = fast.iter().chain(slow.iter()).copied().collect();
    truth.sort_by(f64::total_cmp);
    let q = |p: f64| truth[((truth.len() as f64 * p) as usize).min(truth.len() - 1)];
    eprintln!(
        "ground truth merged (nearest-rank): p50={:.6}s p90={:.6}s p99={:.6}s",
        q(0.5),
        q(0.9),
        q(0.99)
    );

    for profile in [HistogramProfile::Le, HistogramProfile::VmRange] {
        let mut config = SnapshotConfig::default();
        config.histogram_profile = profile;
        let cache = SnapshotCache::with_config(MetricTreeSnapshot::new(Arc::clone(&tree)), config);
        cache.refresh().expect("render");
        let body = cache.snapshot().expect("snapshot").body();
        println!("### profile {profile:?}");
        println!("{}", String::from_utf8_lossy(&body));
    }
}
