//! Tokio telemetry exported as `metered` metric trees.
//!
//! This crate is for *Tokio's own metrics* (task and runtime telemetry), not for
//! running a metrics HTTP server. Import the metric tree you need and register it
//! alongside your service metrics:
//!
//! ```
//! use metered::Registry;
//! use metered_om::{OpenMetricsEncoder, OpenMetricsRegistryExt};
//! use metered_telemetry_tokio::TokioTaskMetrics;
//!
//! let task_metrics = TokioTaskMetrics::new();
//! let mut registry = Registry::new();
//! registry.register(
//!     metered::entry::metric("tokio_task")
//!         .source(&task_metrics)
//!         .help("Tokio task metrics"),
//! );
//! let text = registry.encode_to_string().unwrap();
//! assert!(text.contains("# TYPE tokio_task_instrumented counter"));
//! ```
//!
//! Wrap spawned futures with [`TokioTaskMetrics::instrument`] to feed the task
//! metrics. Runtime-wide metrics from `tokio::runtime::Handle::metrics()` are
//! available as `TokioRuntimeMetrics` when compiled with `--cfg tokio_unstable`.

use metered::{join_name, MetricSchema, MetricTree, MetricType, MetricValues};
use std::future::Future;
use std::time::Duration;
pub use tokio_metrics::{Instrumented, TaskMonitor};

#[cfg(tokio_unstable)]
mod runtime;

#[cfg(tokio_unstable)]
pub use runtime::TokioRuntimeMetrics;

/// Tokio task telemetry backed by [`tokio_metrics::TaskMonitor`].
#[derive(Debug)]
pub struct TokioTaskMetrics {
    monitor: TaskMonitor,
}

impl TokioTaskMetrics {
    /// Creates a new task monitor metric tree.
    pub fn new() -> Self {
        TokioTaskMetrics {
            monitor: TaskMonitor::new(),
        }
    }

    /// Returns the underlying monitor.
    pub fn monitor(&self) -> &TaskMonitor {
        &self.monitor
    }

    /// Instruments a future so its polling lifecycle contributes to this metric
    /// tree.
    pub fn instrument<F>(&self, future: F) -> Instrumented<F>
    where
        F: Future,
    {
        self.monitor.instrument(future)
    }
}

impl Default for TokioTaskMetrics {
    fn default() -> Self {
        TokioTaskMetrics::new()
    }
}

impl MetricTree for TokioTaskMetrics {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        for family in TASK_COUNTERS {
            let full = join_name(name, family.name);
            schema.set_help_for(&full, family.help);
            if let Some(unit) = family.unit {
                schema.set_unit_for(&full, unit);
            }
            schema.add_family(&full, MetricType::Counter, labels);
        }
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let c = self.monitor.cumulative();
        for family in TASK_COUNTERS {
            values.counter(&join_name(name, family.name), labels, (family.value)(&c));
        }
    }
}

struct TaskCounter {
    name: &'static str,
    help: &'static str,
    // OpenMetrics `# UNIT` (a suffix of `name`); `None` for plain event counts.
    unit: Option<&'static str>,
    value: fn(&tokio_metrics::TaskMetrics) -> u64,
}

fn duration_micros(duration: Duration) -> u64 {
    duration.as_micros().try_into().unwrap_or(u64::MAX)
}

const TASK_COUNTERS: &[TaskCounter] = &[
    TaskCounter {
        name: "instrumented",
        help: "Tasks instrumented.",
        unit: None,
        value: |c| c.instrumented_count,
    },
    TaskCounter {
        name: "dropped",
        help: "Instrumented tasks dropped.",
        unit: None,
        value: |c| c.dropped_count,
    },
    TaskCounter {
        name: "first_poll",
        help: "Tasks polled for the first time.",
        unit: None,
        value: |c| c.first_poll_count,
    },
    TaskCounter {
        name: "total_first_poll_delay_microseconds",
        help: "Total delay between instrumentation and first poll.",
        unit: Some("microseconds"),
        value: |c| duration_micros(c.total_first_poll_delay),
    },
    TaskCounter {
        name: "total_idled",
        help: "Total times tasks went idle awaiting a resource.",
        unit: None,
        value: |c| c.total_idled_count,
    },
    TaskCounter {
        name: "total_idle_duration_microseconds",
        help: "Total time tasks spent idle.",
        unit: Some("microseconds"),
        value: |c| duration_micros(c.total_idle_duration),
    },
    TaskCounter {
        name: "total_scheduled",
        help: "Total times tasks were scheduled to run.",
        unit: None,
        value: |c| c.total_scheduled_count,
    },
    TaskCounter {
        name: "total_scheduled_duration_microseconds",
        help: "Total time tasks spent scheduled before running.",
        unit: Some("microseconds"),
        value: |c| duration_micros(c.total_scheduled_duration),
    },
    TaskCounter {
        name: "total_poll",
        help: "Total times tasks were polled.",
        unit: None,
        value: |c| c.total_poll_count,
    },
    TaskCounter {
        name: "total_poll_duration_microseconds",
        help: "Total time spent polling tasks.",
        unit: Some("microseconds"),
        value: |c| duration_micros(c.total_poll_duration),
    },
    TaskCounter {
        name: "total_fast_poll",
        help: "Total fast polls (below the slow-poll threshold).",
        unit: None,
        value: |c| c.total_fast_poll_count,
    },
    TaskCounter {
        name: "total_fast_poll_duration_microseconds",
        help: "Total time spent in fast polls.",
        unit: Some("microseconds"),
        value: |c| duration_micros(c.total_fast_poll_duration),
    },
    TaskCounter {
        name: "total_slow_poll",
        help: "Total slow polls (at or above the slow-poll threshold).",
        unit: None,
        value: |c| c.total_slow_poll_count,
    },
    TaskCounter {
        name: "total_slow_poll_duration_microseconds",
        help: "Total time spent in slow polls.",
        unit: Some("microseconds"),
        value: |c| duration_micros(c.total_slow_poll_duration),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use metered::Registry;
    use metered_om::OpenMetricsRegistryExt;

    #[tokio::test]
    async fn task_metrics_emit_instrumented_future_counts() {
        let task_metrics = TokioTaskMetrics::new();
        tokio::spawn(task_metrics.instrument(async move {
            tokio::task::yield_now().await;
        }))
        .await
        .unwrap();

        let mut registry = Registry::new();
        registry.register(
            metered::entry::metric("tokio_task")
                .source(&task_metrics)
                .help("Tokio task metrics"),
        );
        let text = registry.encode_to_string().unwrap();

        assert!(text.contains("# TYPE tokio_task_instrumented counter"));
        assert!(text.contains("# HELP tokio_task_instrumented Tasks instrumented."));
        assert!(text.contains("# UNIT tokio_task_total_poll_duration_microseconds microseconds"));
        assert!(text.contains("tokio_task_instrumented_total 1"));
        assert!(text.contains("tokio_task_first_poll_total 1"));
        assert!(text.contains("tokio_task_total_poll_total 2"));
    }

    #[test]
    fn schema_declares_all_task_metric_families() {
        let task_metrics = TokioTaskMetrics::new();
        let mut schema = MetricSchema::new();
        task_metrics.describe("tokio_task", &[], &mut schema);

        assert!(schema.family("tokio_task_instrumented").is_some());
        assert!(schema
            .family("tokio_task_total_scheduled_duration_microseconds")
            .is_some());
        assert_eq!(schema.families().len(), TASK_COUNTERS.len());
    }
}
