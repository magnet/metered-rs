//! Standard process telemetry exported as a [`metered`] metric tree.
//!
//! Exposes the canonical Prometheus process metrics
//! ([`process_cpu_seconds_total`](https://prometheus.io/docs/instrumenting/writing_clientlibs/#process-metrics),
//! `process_resident_memory_bytes`, `process_open_fds`, ...) for the running
//! process, sampled cross-platform via the [`metrics-process`] collector. Mount
//! it under the conventional `process` name so the families come out with their
//! well-known names:
//!
//! ```
//! use metered::Registry;
//! use metered_om::{OpenMetricsEncoder, OpenMetricsRegistryExt};
//! use metered_telemetry_process::ProcessMetrics;
//!
//! let process = ProcessMetrics::new();
//! let mut registry = Registry::new();
//! registry.register(
//!     metered::entry::metric("process")
//!         .source(&process)
//!         .help("Process resource usage"),
//! );
//! let text = registry.encode_to_string().unwrap();
//! assert!(text.contains("# TYPE process_cpu_seconds counter"));
//! ```
//!
//! ## Async / Tokio
//!
//! [`ProcessMetrics`] is stateless: every scrape samples the OS directly (a fast
//! `/proc` read on Linux, equivalent platform calls elsewhere). The read is
//! cheap and bounded (procfs/sysctl reads allocate a little), so
//! [`collect`](MetricTree::collect) is safe to call from an async scrape
//! handler. There is no background task and nothing to keep warm.
//!
//! [`metrics-process`]: https://crates.io/crates/metrics-process

use metered::{
    Help, MetricSampleValue, MetricSchema, MetricTree, MetricType, MetricValues, join_name,
};
use metrics_process::collector::{self, Metrics};

/// Process telemetry (CPU time, memory, file descriptors, threads, start time).
///
/// Implements [`MetricTree`] by sampling the running process on each scrape.
/// Only the metrics the current platform supports are emitted; the rest are
/// silently skipped.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessMetrics {
    _private: (),
}

impl ProcessMetrics {
    /// Creates a process metrics tree for the running process.
    pub fn new() -> Self {
        ProcessMetrics::default()
    }

    /// Samples the current process metrics directly, for callers that want the
    /// raw snapshot without going through the metric tree.
    pub fn sample(&self) -> Metrics {
        collector::collect()
    }
}

impl MetricTree for ProcessMetrics {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        for metric in PROCESS_METRICS {
            let full = join_name(name, metric.name);
            schema.set_help_for(&full, Help::from(metric.help));
            schema.add_family(&full, metric.kind, labels);
        }
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        let metrics = collector::collect();
        for metric in PROCESS_METRICS {
            let Some(value) = (metric.value)(&metrics) else {
                continue;
            };
            let full = join_name(name, metric.name);
            match metric.kind {
                MetricType::Counter => values.counter(&full, labels, value),
                _ => values.gauge(&full, labels, value),
            }
        }
    }
}

struct ProcessMetric {
    name: &'static str,
    kind: MetricType,
    help: &'static str,
    value: fn(&Metrics) -> Option<MetricSampleValue>,
}

const PROCESS_METRICS: &[ProcessMetric] = &[
    ProcessMetric {
        name: "cpu_seconds",
        kind: MetricType::Counter,
        help: "Total user and system CPU time spent in seconds.",
        value: |m| m.cpu_seconds_total.map(MetricSampleValue::from),
    },
    ProcessMetric {
        name: "open_fds",
        kind: MetricType::Gauge,
        help: "Number of open file descriptors.",
        value: |m| m.open_fds.map(MetricSampleValue::from),
    },
    ProcessMetric {
        name: "max_fds",
        kind: MetricType::Gauge,
        help: "Maximum number of open file descriptors.",
        value: |m| m.max_fds.map(MetricSampleValue::from),
    },
    ProcessMetric {
        name: "virtual_memory_bytes",
        kind: MetricType::Gauge,
        help: "Virtual memory size in bytes.",
        value: |m| m.virtual_memory_bytes.map(MetricSampleValue::from),
    },
    ProcessMetric {
        name: "virtual_memory_max_bytes",
        kind: MetricType::Gauge,
        help: "Maximum amount of virtual memory available in bytes.",
        value: |m| m.virtual_memory_max_bytes.map(MetricSampleValue::from),
    },
    ProcessMetric {
        name: "resident_memory_bytes",
        kind: MetricType::Gauge,
        help: "Resident memory size in bytes.",
        value: |m| m.resident_memory_bytes.map(MetricSampleValue::from),
    },
    ProcessMetric {
        name: "start_time_seconds",
        kind: MetricType::Gauge,
        help: "Start time of the process since the Unix epoch in seconds.",
        value: |m| m.start_time_seconds.map(MetricSampleValue::from),
    },
    ProcessMetric {
        name: "threads",
        kind: MetricType::Gauge,
        help: "Number of OS threads in the process.",
        value: |m| m.threads.map(MetricSampleValue::from),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use metered::Registry;
    use metered_om::{OpenMetricsDocument, OpenMetricsRegistryExt};

    #[test]
    fn process_metrics_render_under_the_process_prefix() {
        let process = ProcessMetrics::new();
        let mut registry = Registry::new();
        registry.register(
            metered::entry::metric("process")
                .source(&process)
                .help("Process resource usage"),
        );
        let text = registry.encode_to_string().unwrap();

        // The counter family is declared with the OpenMetrics base name; the
        // sample carries the `_total` suffix.
        assert!(text.contains("# TYPE process_cpu_seconds counter"));

        let doc = OpenMetricsDocument::parse(&text).unwrap();
        // At least one standard family is always present on a supported platform.
        assert!(
            doc.family("process_cpu_seconds").is_some()
                || doc.family("process_resident_memory_bytes").is_some(),
            "expected at least one standard process family:\n{text}"
        );

        // Platform-dependent fields are skipped at collect time (`Option` in
        // the collector), so mirror that: of the samples the platform *does*
        // report, at least one anchor must exist and every reported anchor
        // must be genuinely positive -- a zeroed or broken collector fails.
        let anchors = [
            "process_start_time_seconds",
            "process_resident_memory_bytes",
        ];
        let mut reported = 0;
        for name in anchors {
            let Some(sample) = doc.sample(name) else {
                continue;
            };
            reported += 1;
            let value: f64 = sample
                .value
                .parse()
                .unwrap_or_else(|_| panic!("{name} sample is not numeric: {:?}", sample.value));
            assert!(
                value > 0.0,
                "{name} should be positive, got {value}:\n{text}"
            );
        }
        assert!(
            reported > 0,
            "expected at least one positive anchor sample ({anchors:?}):\n{text}"
        );
    }

    #[test]
    fn schema_declares_every_standard_family() {
        let process = ProcessMetrics::new();
        let mut schema = MetricSchema::new();
        process.describe("process", &[], &mut schema);

        for metric in PROCESS_METRICS {
            let full = join_name("process", metric.name);
            assert!(
                schema.family(&full).is_some(),
                "missing declared family {full}"
            );
        }
        assert_eq!(schema.families().len(), PROCESS_METRICS.len());
    }
}
