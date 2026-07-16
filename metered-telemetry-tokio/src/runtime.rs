//! Tokio runtime-wide telemetry (`--cfg tokio_unstable`).

use metered::{join_name, MetricSchema, MetricTree, MetricType, MetricValues};
use tokio::runtime::{Handle, RuntimeMetrics};

/// Tokio runtime telemetry from [`tokio::runtime::Handle::metrics`].
#[derive(Clone, Debug)]
pub struct TokioRuntimeMetrics {
    handle: Handle,
}

impl TokioRuntimeMetrics {
    /// Uses the current runtime handle.
    ///
    /// Panics with Tokio's normal `Handle::current` behavior if called outside a
    /// Tokio runtime.
    pub fn current() -> Self {
        TokioRuntimeMetrics {
            handle: Handle::current(),
        }
    }

    /// Uses an explicit runtime handle.
    pub fn new(handle: Handle) -> Self {
        TokioRuntimeMetrics { handle }
    }

    fn metrics(&self) -> RuntimeMetrics {
        self.handle.metrics()
    }
}

impl MetricTree for TokioRuntimeMetrics {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        for family in RUNTIME_GAUGES {
            let full = join_name(name, family.name);
            schema.set_help_for(&full, family.help);
            schema.add_family(&full, MetricType::Gauge, labels);
        }
        for family in RUNTIME_COUNTERS {
            let full = join_name(name, family.name);
            schema.set_help_for(&full, family.help);
            schema.add_family(&full, MetricType::Counter, labels);
        }
        let worker_labels = with_worker_label(labels, "");
        for family in WORKER_GAUGES {
            let full = join_name(name, family.name);
            schema.set_help_for(&full, family.help);
            schema.add_family(&full, MetricType::Gauge, &worker_labels);
        }
        for family in WORKER_COUNTERS {
            let full = join_name(name, family.name);
            schema.set_help_for(&full, family.help);
            if let Some(unit) = family.unit {
                schema.set_unit_for(&full, unit);
            }
            schema.add_family(&full, MetricType::Counter, &worker_labels);
        }
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let metrics = self.metrics();
        for family in RUNTIME_GAUGES {
            values.gauge(
                &join_name(name, family.name),
                labels,
                (family.value)(&metrics),
            );
        }
        for family in RUNTIME_COUNTERS {
            values.counter(
                &join_name(name, family.name),
                labels,
                (family.value)(&metrics),
            );
        }

        for worker in 0..metrics.num_workers() {
            let worker_label = worker.to_string();
            let worker_labels = with_worker_label(labels, worker_label.as_str());
            for family in WORKER_GAUGES {
                values.gauge(
                    &join_name(name, family.name),
                    &worker_labels,
                    (family.value)(&metrics, worker),
                );
            }
            for family in WORKER_COUNTERS {
                values.counter(
                    &join_name(name, family.name),
                    &worker_labels,
                    (family.value)(&metrics, worker),
                );
            }
        }
    }
}

fn with_worker_label<'a>(
    labels: &[(&'a str, &'a str)],
    worker: &'a str,
) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::with_capacity(labels.len() + 1);
    out.extend_from_slice(labels);
    out.push(("worker", worker));
    out
}

struct RuntimeGauge {
    name: &'static str,
    help: &'static str,
    value: fn(&RuntimeMetrics) -> i64,
}

struct RuntimeCounter {
    name: &'static str,
    help: &'static str,
    value: fn(&RuntimeMetrics) -> u64,
}

struct WorkerGauge {
    name: &'static str,
    help: &'static str,
    value: fn(&RuntimeMetrics, usize) -> i64,
}

struct WorkerCounter {
    name: &'static str,
    help: &'static str,
    // OpenMetrics `# UNIT` (a suffix of `name`); `None` for plain event counts.
    unit: Option<&'static str>,
    value: fn(&RuntimeMetrics, usize) -> u64,
}

const RUNTIME_GAUGES: &[RuntimeGauge] = &[
    RuntimeGauge {
        name: "num_workers",
        help: "Worker threads used by the runtime.",
        value: |m| m.num_workers() as i64,
    },
    RuntimeGauge {
        name: "num_blocking_threads",
        help: "Additional blocking threads spawned by the runtime.",
        value: |m| m.num_blocking_threads() as i64,
    },
    RuntimeGauge {
        name: "num_idle_blocking_threads",
        help: "Idle blocking threads spawned by the runtime.",
        value: |m| m.num_idle_blocking_threads() as i64,
    },
    RuntimeGauge {
        name: "active_tasks",
        help: "Alive tasks in the runtime.",
        value: |m| m.num_alive_tasks() as i64,
    },
    RuntimeGauge {
        name: "injection_queue_depth",
        help: "Tasks queued in the runtime's global injection queue.",
        value: |m| m.global_queue_depth() as i64,
    },
    RuntimeGauge {
        name: "blocking_queue_depth",
        help: "Tasks queued for the blocking thread pool.",
        value: |m| m.blocking_queue_depth() as i64,
    },
];

const RUNTIME_COUNTERS: &[RuntimeCounter] = &[
    RuntimeCounter {
        name: "io_driver_fd_deregistered",
        help: "File descriptors deregistered from the I/O driver.",
        value: |m| m.io_driver_fd_deregistered_count(),
    },
    RuntimeCounter {
        name: "io_driver_fd_registered",
        help: "File descriptors registered with the I/O driver.",
        value: |m| m.io_driver_fd_registered_count(),
    },
    RuntimeCounter {
        name: "io_driver_ready",
        help: "Readiness events received by the I/O driver.",
        value: |m| m.io_driver_ready_count(),
    },
    RuntimeCounter {
        name: "remote_schedule",
        help: "Tasks scheduled from outside the runtime.",
        value: |m| m.remote_schedule_count(),
    },
    RuntimeCounter {
        name: "budget_forced_yield",
        help: "Times a task was forced to yield after exhausting its budget.",
        value: |m| m.budget_forced_yield_count(),
    },
];

const WORKER_GAUGES: &[WorkerGauge] = &[WorkerGauge {
    name: "worker_local_queue_depth",
    help: "Tasks queued in a worker's local run queue.",
    value: |m, w| m.worker_local_queue_depth(w) as i64,
}];

const WORKER_COUNTERS: &[WorkerCounter] = &[
    WorkerCounter {
        name: "worker_local_schedule",
        help: "Tasks scheduled to a worker's local run queue.",
        unit: None,
        value: |m, w| m.worker_local_schedule_count(w),
    },
    WorkerCounter {
        name: "worker_noop",
        help: "Times a worker unparked but found no work to run.",
        unit: None,
        value: |m, w| m.worker_noop_count(w),
    },
    WorkerCounter {
        name: "worker_overflow",
        help: "Tasks a worker moved to the global queue on local-queue overflow.",
        unit: None,
        value: |m, w| m.worker_overflow_count(w),
    },
    WorkerCounter {
        name: "worker_park",
        help: "Times a worker parked.",
        unit: None,
        value: |m, w| m.worker_park_count(w),
    },
    WorkerCounter {
        name: "worker_poll",
        help: "Tasks polled by a worker.",
        unit: None,
        value: |m, w| m.worker_poll_count(w),
    },
    WorkerCounter {
        name: "worker_steal",
        help: "Tasks a worker stole from other workers.",
        unit: None,
        value: |m, w| m.worker_steal_count(w),
    },
    WorkerCounter {
        name: "worker_total_busy_microseconds",
        help: "Total time a worker spent busy.",
        unit: Some("microseconds"),
        value: |m, w| m.worker_total_busy_duration(w).as_micros() as u64,
    },
];
