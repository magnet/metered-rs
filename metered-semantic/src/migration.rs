//! Migration mode: expose a legacy summary shape alongside the native histogram.
//!
//! When a service moves from a summary-shaped latency metric (pre-computed
//! quantiles, the shape the legacy HDR metrics emitted) to a
//! native cumulative `BucketHistogram`, its existing dashboards
//! still query `name{quantile="..."}`, `name_sum`, `name_count`. Flipping the
//! shape in one deploy breaks those panels.
//!
//! This module closes that gap. [`LegacySummary`] borrows a histogram (a
//! `BucketHistogram` or an [`Elapsed`]) and
//! exposes it as an OpenMetrics **summary**, computing the quantiles from the
//! histogram buckets (the same way `histogram_quantile()` would, but
//! server-side). You register it under the *old* metric name, register the
//! histogram under the *new* name, and both shapes are emitted from the same
//! recorded data. Once the dashboards are migrated, drop the
//! `migration` feature and the legacy registration.
//!
//! ```
//! use metered::{BucketHistogram, Registry};
//! use metered_semantic::migration::LegacySummary;
//! use metered_om::OpenMetricsRegistryExt;
//!
//! // The application's single source of truth -- the new bucket histogram.
//! let latency = BucketHistogram::default();
//! latency.observe(0.012);
//! latency.observe(0.4);
//!
//! // Old shape, old name -- derived from the very same histogram.
//! let legacy = LegacySummary::new(&latency);
//!
//! let mut registry = Registry::new();
//! // New shape, new name.
//! registry.register(
//!     metered::entry::metric("http_request_duration_seconds")
//!         .source(&latency)
//!         .help("Request latency"),
//! );
//! registry.register(
//!     metered::entry::metric("response_time")
//!         .source(&legacy)
//!         .help("Legacy latency summary"),
//! );
//!
//! let text = registry.encode_to_string().unwrap();
//! assert!(text.contains("# TYPE http_request_duration_seconds histogram"));
//! assert!(text.contains("# TYPE response_time summary"));
//! assert!(text.contains("response_time{quantile=\"0.99\"}"));
//! assert!(text.contains("response_time_count 2"));
//! ```
//!
//! Quantiles read off a bucket histogram are estimates (bounded by the bucket
//! resolution) and, unlike the native histogram, summaries **do not aggregate**
//! across replicas. That is acceptable for a temporary migration view; it is not
//! a metric you should keep.

use crate::metric::Measure;
use crate::Elapsed;
use metered::bucket_histogram::{ExemplarSource, HistogramSnapshot};
use metered::metric_tree::{Metric, MetricTree, MetricType};
use metered::schema::MetricSchema;
use metered::summary::{
    quantile_from_buckets, valid_quantiles, QuantileSource, Summary, SummaryReading,
};
use metered::values::MetricValues;
use metered::Histogram;
use parking_lot::Mutex;
use std::ops::Deref;

/// The legacy quantiles the old HDR-backed summaries exposed. Used by default
/// so existing panels keep resolving.
pub const LEGACY_QUANTILES: [f64; 4] = [0.9, 0.95, 0.99, 0.999];

/// A clearable baseline for a [`LegacySummary`].
///
/// The native histogram is cumulative and never resets. Some legacy tooling,
/// though, expects to be able to *clear* a summary (the HDR summaries supported
/// it). A `SummaryWindow` records a baseline snapshot so a windowed
/// `LegacySummary` reports values *since the last clear* rather than for all
/// time, without disturbing the histogram itself.
///
/// This exists only to ease migration; once dashboards no longer expect a
/// clearable summary, drop it.
#[derive(Default)]
pub struct SummaryWindow {
    baseline: Mutex<Option<HistogramSnapshot>>,
}

impl SummaryWindow {
    /// Creates a window with no baseline (the summary reports cumulative values).
    pub fn new() -> Self {
        SummaryWindow::default()
    }

    /// Clears the window: records `source`'s current snapshot as the new
    /// baseline, so subsequent summaries report only what is observed afterwards.
    pub fn clear(&self, source: &impl Histogram) {
        *self.baseline.lock() = Some(source.snapshot());
    }

    /// Clears the window from an [`Elapsed`] metric. `Elapsed` is a measuring
    /// wrapper, not a histogram backend, so it is intentionally explicit rather
    /// than part of the generic histogram API.
    pub fn clear_elapsed<S: ExemplarSource>(&self, source: &Elapsed<S>) {
        *self.baseline.lock() = Some(source.snapshot());
    }

    /// Drops the baseline, returning the summary to reporting cumulative values.
    pub fn reset(&self) {
        *self.baseline.lock() = None;
    }

    fn baseline(&self) -> Option<HistogramSnapshot> {
        self.baseline.lock().clone()
    }
}

/// A [`QuantileSource`] over a borrowed histogram (or [`Elapsed`]) snapshot that
/// applies the legacy windowing/delta logic. It reads the snapshot once per
/// [`read_summary`](QuantileSource::read_summary), so the reported quantiles,
/// `sum`, and `count` are mutually consistent.
struct WindowedQuantiles<'a> {
    snapshot: Box<dyn Fn() -> HistogramSnapshot + 'a>,
    window: Option<&'a SummaryWindow>,
}

impl WindowedQuantiles<'_> {
    /// The cumulative buckets, sum, and count for the current view: either the
    /// raw snapshot or, when windowed, the delta since the last clear.
    fn current(&self) -> (Vec<(f64, u64)>, f64, u64) {
        let snapshot = (self.snapshot)();
        match self.window.and_then(SummaryWindow::baseline) {
            Some(baseline) => delta(&snapshot, &baseline),
            None => {
                let buckets = snapshot
                    .buckets
                    .iter()
                    .map(|bucket| (bucket.le, bucket.cumulative_count))
                    .collect();
                (buckets, snapshot.sum, snapshot.count)
            }
        }
    }
}

impl QuantileSource for WindowedQuantiles<'_> {
    fn read_summary(&self, quantiles: &[f64]) -> SummaryReading {
        let (buckets, sum, count) = self.current();
        SummaryReading {
            values: quantiles
                .iter()
                .map(|&q| quantile_from_buckets(q, &buckets, count))
                .collect(),
            sum,
            count,
        }
    }
}

/// A summary view over a native histogram, for migration.
///
/// Borrows a histogram source and exposes it as an OpenMetrics summary
/// (`name{quantile="q"}`, `name_sum`, `name_count`). Construct one at scrape
/// time and register it under the legacy metric name; see the
/// [module docs](crate::migration).
///
/// It is a thin wrapper over a [`Summary`] whose [`QuantileSource`] reads the
/// borrowed histogram (applying any [`SummaryWindow`]); the summary rendering
/// itself is shared with [`Summary`].
pub struct LegacySummary<'a> {
    inner: Summary<WindowedQuantiles<'a>>,
}

impl<'a> LegacySummary<'a> {
    /// A cumulative summary over `source`, using the [`LEGACY_QUANTILES`].
    pub fn new(source: &'a impl Histogram) -> Self {
        Self::over(Box::new(move || source.snapshot()), None)
    }

    /// A cumulative summary over an [`Elapsed`] metric.
    ///
    /// `Elapsed` is a measured-duration wrapper, not a histogram backend, so it
    /// has an explicit constructor rather than implementing [`Histogram`].
    pub fn from_elapsed<S: ExemplarSource>(source: &'a Elapsed<S>) -> Self {
        Self::over(Box::new(move || source.snapshot()), None)
    }

    /// A summary over `source` that reports values since `window` was last
    /// cleared (see [`SummaryWindow`]).
    pub fn windowed(source: &'a impl Histogram, window: &'a SummaryWindow) -> Self {
        Self::over(Box::new(move || source.snapshot()), Some(window))
    }

    /// A windowed summary over an [`Elapsed`] metric.
    pub fn windowed_elapsed<S: ExemplarSource>(
        source: &'a Elapsed<S>,
        window: &'a SummaryWindow,
    ) -> Self {
        Self::over(Box::new(move || source.snapshot()), Some(window))
    }

    /// Overrides the reported quantiles. Out-of-range values (outside `0.0 ..=
    /// 1.0`) are dropped.
    pub fn with_quantiles(mut self, quantiles: impl IntoIterator<Item = f64>) -> Self {
        self.inner = self.inner.with_quantiles(quantiles);
        self
    }

    fn over(
        snapshot: Box<dyn Fn() -> HistogramSnapshot + 'a>,
        window: Option<&'a SummaryWindow>,
    ) -> Self {
        let source = WindowedQuantiles { snapshot, window };
        LegacySummary {
            inner: Summary::new(source).with_quantiles(LEGACY_QUANTILES),
        }
    }
}

impl Metric for LegacySummary<'_> {
    fn metric_type(&self) -> MetricType {
        self.inner.metric_type()
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.inner.collect_metric(name, labels, values);
    }

    fn describe_metric(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.inner.describe_metric(name, labels, schema);
    }
}

/// Subtracts `baseline` from `current`, yielding cumulative bucket counts, sum,
/// and count for the window since the baseline was taken. Both snapshots come
/// from the same histogram, so their bucket boundaries line up.
fn delta(current: &HistogramSnapshot, baseline: &HistogramSnapshot) -> (Vec<(f64, u64)>, f64, u64) {
    let buckets = current
        .buckets
        .iter()
        .zip(&baseline.buckets)
        .map(|(cur, base)| {
            (
                cur.le,
                cur.cumulative_count.saturating_sub(base.cumulative_count),
            )
        })
        .collect();
    (
        buckets,
        current.sum - baseline.sum,
        current.count.saturating_sub(baseline.count),
    )
}

/// A histogram metric that also emits a legacy summary, from a single
/// registration.
///
/// Hold one of these in place of a `BucketHistogram`
/// or [`Elapsed`]: it [`Deref`]s to the inner metric (so
/// `observe` / `measure!` / `#[metered]` usage is unchanged) and implements
/// [`MetricTree`] so registering it once emits *both* the
/// native histogram (under the registered name) and a [`LegacySummary`] (under
/// the configured legacy name).
///
/// ```
/// use metered::{BucketHistogram, Registry};
/// use metered_semantic::migration::WithLegacySummary;
/// use metered_om::OpenMetricsRegistryExt;
///
/// // Was `latency: BucketHistogram`; now also carries its legacy summary.
/// let latency = WithLegacySummary::new(BucketHistogram::default(), "response_time");
/// latency.observe(0.012); // instrumentation is unchanged (via Deref)
///
/// let mut registry = Registry::new();
/// // One registration, both shapes.
/// registry.register(
///     metered::entry::metric("http_request_duration_seconds")
///         .source(&latency)
///         .help("Request latency"),
/// );
///
/// let text = registry.encode_to_string().unwrap();
/// assert!(text.contains("# TYPE http_request_duration_seconds histogram"));
/// assert!(text.contains("# TYPE response_time summary"));
/// ```
///
/// The legacy name is an absolute family name: it is **not** affected by a
/// `Registry` prefix (the wrapper only sees the already-built
/// name of the native metric). Pass the exact legacy series name your dashboards
/// query, including any prefix it historically had.
pub struct WithLegacySummary<H> {
    inner: H,
    legacy_name: String,
    legacy_help: Option<String>,
    quantiles: Vec<f64>,
    window: Option<SummaryWindow>,
}

impl<H> WithLegacySummary<H> {
    /// Wraps `inner`, emitting a cumulative legacy summary under `legacy_name`
    /// with the [`LEGACY_QUANTILES`].
    pub fn new(inner: H, legacy_name: impl Into<String>) -> Self {
        WithLegacySummary {
            inner,
            legacy_name: legacy_name.into(),
            legacy_help: None,
            quantiles: LEGACY_QUANTILES.to_vec(),
            window: None,
        }
    }

    /// Sets the `# HELP` text for the legacy summary family.
    pub fn legacy_help(mut self, help: impl Into<String>) -> Self {
        self.legacy_help = Some(help.into());
        self
    }

    /// Overrides the summary quantiles (out-of-range values are dropped).
    pub fn with_quantiles(mut self, quantiles: impl IntoIterator<Item = f64>) -> Self {
        self.quantiles = valid_quantiles(quantiles);
        self
    }

    /// Gives the legacy summary a clearable [`SummaryWindow`], so it reports
    /// values since the last [`clear_window`](WithLegacySummary::clear_window)
    /// rather than cumulatively.
    pub fn windowed(mut self) -> Self {
        self.window = Some(SummaryWindow::new());
        self
    }

    /// The wrapped metric.
    pub fn inner(&self) -> &H {
        &self.inner
    }

    /// Unwraps, returning the inner metric.
    pub fn into_inner(self) -> H {
        self.inner
    }

    fn legacy_summary(&self) -> LegacySummary<'_>
    where
        H: Histogram,
    {
        let summary = match &self.window {
            Some(window) => LegacySummary::windowed(&self.inner, window),
            None => LegacySummary::new(&self.inner),
        };
        summary.with_quantiles(self.quantiles.iter().copied())
    }
}

impl<H: Histogram> WithLegacySummary<H> {
    /// Clears the legacy summary window (no-op unless built with
    /// [`windowed`](WithLegacySummary::windowed)): records the current snapshot
    /// as the baseline so the summary reports only subsequent observations.
    pub fn clear_window(&self) {
        if let Some(window) = &self.window {
            window.clear(&self.inner);
        }
    }

    /// Drops the window baseline, returning the summary to cumulative values.
    pub fn reset_window(&self) {
        if let Some(window) = &self.window {
            window.reset();
        }
    }
}

impl<H> Deref for WithLegacySummary<H> {
    type Target = H;

    fn deref(&self) -> &H {
        &self.inner
    }
}

impl<H: Measure> Measure for WithLegacySummary<H> {
    type Recorder = H::Recorder;

    fn enter(&self) -> Self::Recorder {
        self.inner.enter()
    }
}

impl<H: MetricTree + Histogram> MetricTree for WithLegacySummary<H> {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.inner.describe(name, labels, schema);
        if let Some(help) = self.legacy_help.clone() {
            schema.set_help_for(&self.legacy_name, help);
        }
        self.legacy_summary()
            .describe_metric(&self.legacy_name, labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.inner.collect(name, labels, values);
        self.legacy_summary()
            .collect_metric(&self.legacy_name, labels, values);
    }

    fn housekeep(&self) {
        self.inner.housekeep();
    }

    fn needs_housekeep(&self) -> bool {
        self.inner.needs_housekeep()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Elapsed;
    use metered::bucket_histogram::Buckets;
    use metered::{BucketHistogram, MetricTree};

    // Dual-shape (native + legacy summary) OpenMetrics *rendering* is tested in
    // `metered-om/tests/render_migration.rs`; here we test the summary
    // derivation (quantile math, windowing) via the schema/values model.

    fn histogram_with(observations: &[f64]) -> BucketHistogram {
        let histogram = BucketHistogram::new(Buckets::custom([1.0, 2.0, 3.0, 4.0]));
        for &value in observations {
            histogram.observe(value);
        }
        histogram
    }

    #[test]
    fn legacy_summary_emits_quantiles_sum_and_count() {
        let histogram = histogram_with(&[0.5, 1.5, 1.5, 2.5, 3.5]);
        let summary = LegacySummary::new(&histogram);

        let mut values = MetricValues::new();
        summary.collect("response_time", &[("svc", "api")], &mut values);

        let count = values
            .samples()
            .iter()
            .find(|s| s.name == "response_time_count")
            .unwrap();
        assert_eq!(count.value.to_string(), "5");
        assert!(values
            .samples()
            .iter()
            .any(|s| s.name == "response_time_sum"));
        // One sample per legacy quantile, all carrying the quantile label.
        let quantile_samples: Vec<_> = values
            .samples()
            .iter()
            .filter(|s| s.name == "response_time")
            .collect();
        assert_eq!(quantile_samples.len(), LEGACY_QUANTILES.len());
        assert!(quantile_samples
            .iter()
            .all(|s| s.labels.iter().any(|(k, _)| k == "quantile")));
    }

    #[test]
    fn describe_declares_a_summary_with_a_quantile_label() {
        let histogram = histogram_with(&[1.5]);
        let summary = LegacySummary::new(&histogram);
        let mut schema = MetricSchema::new();
        summary.describe("response_time", &[("svc", "api")], &mut schema);

        let family = schema.family("response_time").unwrap();
        assert_eq!(family.metric_type, MetricType::Summary);
        assert!(family.labels.iter().any(|l| l == "quantile"));
        assert!(family.labels.iter().any(|l| l == "svc"));
    }

    #[test]
    fn windowed_summary_reports_values_since_clear() {
        let histogram = histogram_with(&[1.5, 1.5, 1.5]);
        let window = SummaryWindow::new();
        window.clear(&histogram); // baseline at 3 observations

        // More traffic after the clear.
        histogram.observe(2.5);
        histogram.observe(2.5);

        let summary = LegacySummary::windowed(&histogram, &window);
        let mut values = MetricValues::new();
        summary.collect("response_time", &[], &mut values);

        let count = values
            .samples()
            .iter()
            .find(|s| s.name == "response_time_count")
            .unwrap();
        assert_eq!(count.value.to_string(), "2", "only post-clear observations");
        let sum = values
            .samples()
            .iter()
            .find(|s| s.name == "response_time_sum")
            .unwrap();
        assert_eq!(sum.value.to_string(), "5"); // 2.5 + 2.5

        // Resetting the window restores the cumulative view.
        window.reset();
        let mut cumulative = MetricValues::new();
        LegacySummary::windowed(&histogram, &window).collect("response_time", &[], &mut cumulative);
        let count = cumulative
            .samples()
            .iter()
            .find(|s| s.name == "response_time_count")
            .unwrap();
        assert_eq!(count.value.to_string(), "5");
    }

    #[test]
    fn out_of_range_quantiles_are_dropped() {
        let histogram = histogram_with(&[1.5]);
        let summary = LegacySummary::new(&histogram).with_quantiles([0.5, 1.5, -0.1]);
        let mut values = MetricValues::new();
        summary.collect("response_time", &[], &mut values);
        assert_eq!(
            values
                .samples()
                .iter()
                .filter(|s| s.name == "response_time")
                .count(),
            1
        );
    }

    #[test]
    fn wrapper_derefs_to_inner_for_instrumentation() {
        let latency = WithLegacySummary::new(BucketHistogram::default(), "legacy");
        latency.observe(0.5);
        latency.observe(1.5);
        // `snapshot`/`observe` come from the inner histogram via Deref.
        assert_eq!(latency.snapshot().count, 2);
        assert_eq!(latency.inner().count(), 2);
    }

    #[test]
    fn legacy_summary_can_read_elapsed_explicitly() {
        let elapsed = Elapsed::<metered::NoExemplars>::default();
        {
            // The recorder records the elapsed observation on drop.
            let _recorder = elapsed.enter();
        }
        let summary = LegacySummary::from_elapsed(&elapsed);
        let mut values = MetricValues::new();
        summary.collect("legacy", &[], &mut values);
        assert!(values
            .samples()
            .iter()
            .any(|sample| sample.name == "legacy_count" && sample.value.to_string() == "1"));
    }

    #[test]
    fn windowed_wrapper_summary_reports_since_clear() {
        let wrapped =
            WithLegacySummary::new(histogram_with(&[1.5, 1.5, 1.5]), "legacy_summary").windowed();
        wrapped.clear_window(); // baseline at 3 observations
        wrapped.observe(2.5); // one more after the clear

        let mut values = MetricValues::new();
        wrapped.collect("native_hist", &[], &mut values);

        // Native histogram is cumulative; legacy summary is windowed. The
        // histogram is carried structurally (the sink renders its buckets), so
        // its count comes from the histogram value, not a flat sample.
        let native = values
            .histograms()
            .iter()
            .find(|h| h.name == "native_hist")
            .map(|h| match &h.data {
                metered::values::HistogramData::Classic(s) => s.count,
                metered::values::HistogramData::Exponential(s) => s.count,
            })
            .unwrap();
        assert_eq!(native, 4);
        let legacy = values
            .samples()
            .iter()
            .find(|s| s.name == "legacy_summary_count")
            .unwrap();
        assert_eq!(legacy.value.to_string(), "1");

        wrapped.reset_window();
        let mut cumulative = MetricValues::new();
        wrapped.collect("native_hist", &[], &mut cumulative);
        let legacy = cumulative
            .samples()
            .iter()
            .find(|s| s.name == "legacy_summary_count")
            .unwrap();
        assert_eq!(legacy.value.to_string(), "4");
    }
}
