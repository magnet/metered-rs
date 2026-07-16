//! [`Metric`] implementations for the built-in leaf types.
//!
//! This is the "how each value type maps to OpenMetrics" layer: counters and
//! gauges, [`Info`] / [`StateSet`], the histograms, and a few standard library
//! atomics. Each maps a value type to a [`MetricType`] and sample set; the
//! concrete wire encoders live in their own crates (e.g. `metered-om`).

use crate::bucket_histogram::BucketHistogram;
use crate::exponential_histogram::{DynamicExponentialHistogram, FixedExponentialHistogram};
use crate::labels::slices::with_labels;
use crate::metric_tree::{Metric, MetricType};
use crate::primitives::{
    AsCounter, AsGauge, CounterSource, GaugeSource, Info, InfoMetric, StateSet,
};
use crate::schema::MetricSchema;
use crate::values::MetricValues;
use crate::Scalar;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};

impl Metric for AtomicBool {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.gauge(name, labels, self.load(Ordering::Relaxed) as i64);
    }
}

impl Metric for AtomicI64 {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.gauge(name, labels, self.load(Ordering::Relaxed));
    }
}

impl Metric for AtomicUsize {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.gauge(name, labels, self.load(Ordering::Relaxed));
    }
}

impl Metric for AtomicU64 {
    fn metric_type(&self) -> MetricType {
        MetricType::Counter
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, self.load(Ordering::Relaxed));
    }
}

impl<T: CounterSource> Metric for AsCounter<T> {
    fn metric_type(&self) -> MetricType {
        MetricType::Counter
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, self.get());
    }
}

impl<T: GaugeSource> Metric for AsGauge<T> {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let value: Scalar = self.get().into();
        values.gauge(name, labels, value);
    }
}

impl Metric for InfoMetric {
    fn metric_type(&self) -> MetricType {
        MetricType::Info
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let info_labels = self.labels();
        let all = with_labels(
            labels,
            info_labels
                .as_slice()
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        );
        values.sample(&format!("{name}_info"), &all, 1u64);
    }

    fn describe_metric(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        let info_labels = self.labels();
        let all = with_labels(
            labels,
            info_labels
                .as_slice()
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
        );
        schema.add_family(name, self.metric_type(), &all);
    }
}

impl Metric for StateSet {
    fn metric_type(&self) -> MetricType {
        MetricType::StateSet
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let active = self.active_index();
        for (index, state) in self.states().iter().enumerate() {
            // The stateset label key is the metric name itself.
            let all = with_labels(labels, [(name, state.as_str())]);
            values.sample(name, &all, u64::from(index == active));
        }
    }

    fn describe_metric(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        let all = with_labels(labels, [(name, "")]);
        schema.add_family(name, self.metric_type(), &all);
    }
}

impl Metric for BucketHistogram {
    fn metric_type(&self) -> MetricType {
        MetricType::Histogram
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.histogram(name, labels, &self.snapshot());
    }
}

impl Metric for FixedExponentialHistogram {
    fn metric_type(&self) -> MetricType {
        MetricType::Histogram
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        // Carry the native snapshot whole; the sink renders it as `le`
        // (cumulative) or `vmrange` (non-cumulative) as configured.
        values.exponential_histogram(name, labels, &self.snapshot());
    }
}

impl Metric for DynamicExponentialHistogram {
    fn metric_type(&self) -> MetricType {
        MetricType::Histogram
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.exponential_histogram(name, labels, &self.snapshot());
    }

    // The off-hot-path downscale and exemplar-window reset are driven by the
    // registry's scrape-time `housekeep` sweep (or a user maintenance task).
    fn needs_housekeep(&self) -> bool {
        DynamicExponentialHistogram::needs_housekeep(self)
    }

    fn housekeep(&self) {
        DynamicExponentialHistogram::housekeep(self)
    }
}
