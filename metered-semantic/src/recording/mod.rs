//! Explicit operation recording for code that does not use tracing spans.

use crate::metric::{Armed, Measure, Recorder};
use metered::bucket_histogram::BucketHistogram;
use metered::handle::Handle;
use metered::metric_tree::{join_name, Metric, MetricTree, MetricType};
use metered::primitives::{Counter, CounterSource, Gauge, GaugeSource};
use metered::schema::MetricSchema;
use metered::values::MetricValues;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

/// A simple set of metrics for explicitly recording one operation.
///
/// `Operation` records how many executions started, returned without aborting,
/// failed, are currently in flight, and how long returned or aborted executions
/// took.
#[derive(Debug, Default)]
pub struct Operation {
    /// Count of operation executions that have started.
    pub started: OperationCounter,
    /// Count of operation executions that returned normally (`Ok` or `Err`).
    pub completed: OperationCounter,
    /// Count of operation executions that returned `Err` or were aborted.
    pub failed: OperationCounter,
    /// Count of operation executions currently in progress.
    pub in_flight: OperationGauge,
    /// Cumulative duration histogram in seconds.
    pub duration: Handle<BucketHistogram>,
}

impl Operation {
    /// Records a `Result`-returning operation and returns its result unchanged.
    pub fn record<T, E>(&self, f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        crate::measure!(self, f())
    }
}

impl MetricTree for Operation {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        self.started
            .describe(&join_name(name, "started"), labels, schema);
        self.completed
            .describe(&join_name(name, "completed"), labels, schema);
        self.failed
            .describe(&join_name(name, "failed"), labels, schema);
        self.in_flight
            .describe(&join_name(name, "in_flight"), labels, schema);
        self.duration
            .describe(&join_name(name, "duration_seconds"), labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        self.started
            .collect(&join_name(name, "started"), labels, values);
        self.completed
            .collect(&join_name(name, "completed"), labels, values);
        self.failed
            .collect(&join_name(name, "failed"), labels, values);
        self.in_flight
            .collect(&join_name(name, "in_flight"), labels, values);
        self.duration
            .collect(&join_name(name, "duration_seconds"), labels, values);
    }

    fn housekeep(&self) {
        MetricTree::housekeep(&*self.duration);
    }

    fn needs_housekeep(&self) -> bool {
        MetricTree::needs_housekeep(&*self.duration)
    }
}

impl Measure for Operation {
    type Recorder = OperationRecorder;

    fn enter(&self) -> Self::Recorder {
        self.started.incr();
        self.in_flight.incr();
        OperationRecorder {
            completed: self.completed.share(),
            failed: self.failed.share(),
            in_flight: self.in_flight.share(),
            duration: self.duration.share(),
            started_at: Instant::now(),
            armed: Armed::new(),
        }
    }
}

/// A monotonically increasing operation counter.
#[derive(Debug, Default)]
pub struct OperationCounter(Handle<AtomicU64>);

impl OperationCounter {
    /// Returns the current counter value.
    pub fn get(&self) -> u64 {
        CounterSource::get(&self.0)
    }

    fn incr(&self) {
        Counter::incr(&self.0);
    }

    fn share(&self) -> Self {
        OperationCounter(self.0.share())
    }
}

impl CounterSource for OperationCounter {
    fn get(&self) -> u64 {
        CounterSource::get(&self.0)
    }
}

impl Counter for OperationCounter {
    fn incr_by(&self, n: u64) {
        self.0.incr_by(n);
    }
}

impl Metric for OperationCounter {
    fn metric_type(&self) -> MetricType {
        MetricType::Counter
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.counter(name, labels, self.get());
    }
}

/// A non-negative operation gauge.
#[derive(Debug, Default)]
pub struct OperationGauge(Handle<AtomicU64>);

impl OperationGauge {
    /// Returns the current gauge value.
    pub fn get(&self) -> u64 {
        GaugeSource::get(&self.0)
    }

    fn incr(&self) {
        Gauge::incr(&self.0);
    }

    fn try_decr(&self) {
        Gauge::try_decr(&self.0);
    }

    fn share(&self) -> Self {
        OperationGauge(self.0.share())
    }
}

impl GaugeSource for OperationGauge {
    type Value = u64;

    fn get(&self) -> Self::Value {
        GaugeSource::get(&self.0)
    }
}

impl Gauge for OperationGauge {
    fn set(&self, value: Self::Value) {
        self.0.set(value);
    }

    fn add(&self, delta: Self::Value) {
        self.0.add(delta);
    }

    fn incr(&self) {
        Gauge::incr(&self.0);
    }

    fn try_decr(&self) -> bool {
        self.0.try_decr()
    }
}

impl Metric for OperationGauge {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.gauge(name, labels, self.get());
    }
}

/// Recorder for a single [`Operation`] execution.
#[derive(Debug)]
pub struct OperationRecorder {
    completed: OperationCounter,
    failed: OperationCounter,
    in_flight: OperationGauge,
    duration: Handle<BucketHistogram>,
    started_at: Instant,
    armed: Armed,
}

impl<T, E> Recorder<Result<T, E>> for OperationRecorder {
    fn complete(&mut self, result: &Result<T, E>) {
        if self.armed.fire() {
            self.completed.incr();
            if result.is_err() {
                self.failed.incr();
            }
            self.in_flight.try_decr();
            self.duration
                .observe(self.started_at.elapsed().as_secs_f64());
        }
    }
}

impl Drop for OperationRecorder {
    fn drop(&mut self) {
        if self.armed.fire() {
            self.failed.incr();
            self.in_flight.try_decr();
            self.duration
                .observe(self.started_at.elapsed().as_secs_f64());
        }
    }
}
