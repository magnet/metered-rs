//! [`Metric`] implementations for the semantic measuring wrappers.
//!
//! These map each semantic wrapper ([`HitCount`], [`ErrorCount`], [`NoneCount`],
//! [`InFlight`], [`Elapsed`]) to a core [`MetricType`] and sample set, so a
//! registry can describe and collect them through the same `metered` model. The
//! `Metric` trait and the leaf encoders for the core types live in `metered`.

use crate::common::{Elapsed, ErrorCount, HitCount, InFlight, NoneCount};
use metered::bucket_histogram::ExemplarSource;
use metered::metric_tree::{Metric, MetricType};
use metered::primitives::CounterSource;
use metered::values::MetricValues;

impl Metric for HitCount {
    fn metric_type(&self) -> MetricType {
        MetricType::Counter
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, CounterSource::get(&self.0));
    }
}

impl Metric for ErrorCount {
    fn metric_type(&self) -> MetricType {
        MetricType::Counter
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, self.get());
    }
}

impl Metric for NoneCount {
    fn metric_type(&self) -> MetricType {
        MetricType::Counter
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, self.get());
    }
}

impl Metric for InFlight {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.gauge(name, labels, self.get());
    }
}

impl<S: ExemplarSource> Metric for Elapsed<S> {
    fn metric_type(&self) -> MetricType {
        MetricType::Histogram
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.histogram(name, labels, &self.snapshot());
    }
}
