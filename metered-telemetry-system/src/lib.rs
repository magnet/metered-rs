//! Host system telemetry exported as a [`metered`] metric tree.
//!
//! Exposes host-level CPU, memory, swap, load average, and uptime for the
//! machine the process runs on, sampled cross-platform via [`sysinfo`]. Mount it
//! under the conventional `system` name:
//!
//! ```
//! use metered::Registry;
//! use metered_om::OpenMetricsRegistryExt;
//! use metered_telemetry_system::SystemMetrics;
//!
//! let system = SystemMetrics::new();
//! let mut registry = Registry::new();
//! registry.register(
//!     metered::entry::metric("system")
//!         .source(&system)
//!         .help("Host system telemetry"),
//! );
//! let text = registry.encode_to_string().unwrap();
//! assert!(text.contains("# TYPE system_memory_total_bytes gauge"));
//! ```
//!
//! ## Async / Tokio
//!
//! Sampling the host (a `sysinfo` refresh of CPU + memory) is synchronous. By
//! default [`collect`](MetricTree::collect) samples inline on each scrape, which
//! is the usual pull-model behaviour and is bounded (sub-millisecond for CPU +
//! memory). CPU utilisation is the average over the interval since the previous
//! sample, so a long-lived `SystemMetrics` scraped on a fixed cadence reports
//! per-scrape-interval CPU.
//!
//! To keep the scrape path free of *any* OS work, enable the `tokio` feature and
//! call `SystemMetrics::spawn`: it drives sampling from a background task
//! (using `spawn_blocking`), and [`collect`](MetricTree::collect) then only reads
//! the cached snapshot — never touching the reactor. The returned
//! `BackgroundSampler` owns the task: hold it for as long as the sampler
//! should run. Dropping it aborts the task, and once the **last** live sampler
//! handle is gone `collect` returns to inline sampling, so a stopped sampler
//! can never leave the scrape serving a frozen snapshot:
//!
//! ```ignore
//! let system = std::sync::Arc::new(SystemMetrics::new());
//! let _sampler = system.clone().spawn(std::time::Duration::from_secs(10));
//! ```
//!
//! [`sysinfo`]: https://crates.io/crates/sysinfo

use metered::{
    Help, MetricSampleValue, MetricSchema, MetricTree, MetricType, MetricValues, join_name,
};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use sysinfo::System;

/// Host system telemetry (CPU, memory, swap, load average, uptime).
///
/// Implements [`MetricTree`]. By default each scrape samples the host inline;
/// while a background sampler is running (`SystemMetrics::spawn`, `tokio`
/// feature), the scrape path reads a cached snapshot instead.
#[derive(Debug)]
pub struct SystemMetrics {
    sampler: Mutex<System>,
    snapshot: Mutex<Option<Snapshot>>,
    /// Live `BackgroundSampler` handles. While non-zero, `collect` reads the
    /// cached snapshot; a count (not a flag) so dropping one of several
    /// samplers cannot flip the scrape back to inline sampling while another
    /// still runs.
    live_samplers: AtomicUsize,
}

impl SystemMetrics {
    /// Creates a host telemetry tree and takes an initial sample (so CPU
    /// utilisation has a baseline to measure the next interval against).
    pub fn new() -> Self {
        let metrics = SystemMetrics {
            sampler: Mutex::new(System::new()),
            snapshot: Mutex::new(None),
            live_samplers: AtomicUsize::new(0),
        };
        metrics.refresh();
        metrics
    }

    /// Samples the host once and stores the result as the current snapshot.
    ///
    /// Synchronous: call it from a background task (see `Self::spawn`)
    /// or rely on the inline sampling [`collect`](MetricTree::collect) does by
    /// default.
    pub fn refresh(&self) {
        let snapshot = {
            let mut system = self.sampler.lock();
            system.refresh_memory();
            system.refresh_cpu_usage();
            let load = System::load_average();
            Snapshot {
                memory_total_bytes: system.total_memory(),
                memory_used_bytes: system.used_memory(),
                memory_available_bytes: system.available_memory(),
                memory_free_bytes: system.free_memory(),
                swap_total_bytes: system.total_swap(),
                swap_used_bytes: system.used_swap(),
                cpu_utilization_ratio: f64::from(system.global_cpu_usage()) / 100.0,
                load1: load.one,
                load5: load.five,
                load15: load.fifteen,
                uptime_seconds: System::uptime(),
            }
        };
        *self.snapshot.lock() = Some(snapshot);
    }

    fn current(&self) -> Option<Snapshot> {
        *self.snapshot.lock()
    }
}

/// The RAII handle owning a background sampling task (`tokio` feature).
///
/// Returned by `SystemMetrics::spawn`. While any sampler handle lives, the
/// scrape path reads the cached snapshot the task(s) refresh; dropping a
/// handle aborts its task, and dropping the **last** one flips
/// [`collect`](MetricTree::collect) back to inline sampling in the same
/// motion, so the two halves of the lifecycle cannot drift apart (no count
/// left claiming a sampler that no longer runs).
#[cfg(feature = "tokio")]
#[derive(Debug)]
pub struct BackgroundSampler {
    metrics: std::sync::Arc<SystemMetrics>,
    task: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "tokio")]
impl Drop for BackgroundSampler {
    fn drop(&mut self) {
        self.task.abort();
        self.metrics.live_samplers.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(feature = "tokio")]
impl SystemMetrics {
    /// Drives sampling from a background Tokio task every `interval`, flipping
    /// [`collect`](MetricTree::collect) to a pure cached read so the scrape path
    /// does no OS work. The synchronous sample runs on a blocking thread.
    ///
    /// Must be called from within a Tokio runtime. Hold the returned
    /// `BackgroundSampler` for as long as background sampling should run;
    /// dropping it stops the task, and dropping the last live sampler returns
    /// the scrape path to inline sampling.
    #[must_use = "dropping the sampler stops background sampling"]
    pub fn spawn(self: std::sync::Arc<Self>, interval: std::time::Duration) -> BackgroundSampler {
        let metrics = std::sync::Arc::clone(&self);
        // Spawn first, then count the lease: if spawn panics (no runtime), the
        // scrape path keeps sampling inline instead of trusting a task that
        // never existed.
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                let this = std::sync::Arc::clone(&self);
                let _ = tokio::task::spawn_blocking(move || this.refresh()).await;
            }
        });
        metrics.live_samplers.fetch_add(1, Ordering::Relaxed);
        BackgroundSampler { metrics, task }
    }
}

impl Default for SystemMetrics {
    fn default() -> Self {
        SystemMetrics::new()
    }
}

impl MetricTree for SystemMetrics {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        for metric in SYSTEM_METRICS {
            let full = join_name(name, metric.name);
            schema.set_help_for(&full, Help::from(metric.help));
            if let Some(unit) = metric.unit {
                schema.set_unit_for(&full, unit);
            }
            schema.add_family(&full, MetricType::Gauge, labels);
        }
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        if self.live_samplers.load(Ordering::Relaxed) == 0 {
            self.refresh();
        }
        let Some(snapshot) = self.current() else {
            return;
        };
        for metric in SYSTEM_METRICS {
            values.gauge(
                &join_name(name, metric.name),
                labels,
                (metric.value)(&snapshot),
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Snapshot {
    memory_total_bytes: u64,
    memory_used_bytes: u64,
    memory_available_bytes: u64,
    memory_free_bytes: u64,
    swap_total_bytes: u64,
    swap_used_bytes: u64,
    cpu_utilization_ratio: f64,
    load1: f64,
    load5: f64,
    load15: f64,
    uptime_seconds: u64,
}

struct SystemMetric {
    name: &'static str,
    help: &'static str,
    // OpenMetrics `# UNIT` (must be a suffix of `name`); `None` for
    // dimensionless metrics such as the load averages.
    unit: Option<&'static str>,
    value: fn(&Snapshot) -> MetricSampleValue,
}

const SYSTEM_METRICS: &[SystemMetric] = &[
    SystemMetric {
        name: "memory_total_bytes",
        help: "Total physical memory in bytes.",
        unit: Some("bytes"),
        value: |s| MetricSampleValue::from(s.memory_total_bytes),
    },
    SystemMetric {
        name: "memory_used_bytes",
        help: "Used physical memory in bytes.",
        unit: Some("bytes"),
        value: |s| MetricSampleValue::from(s.memory_used_bytes),
    },
    SystemMetric {
        name: "memory_available_bytes",
        help: "Available physical memory in bytes.",
        unit: Some("bytes"),
        value: |s| MetricSampleValue::from(s.memory_available_bytes),
    },
    SystemMetric {
        name: "memory_free_bytes",
        help: "Free physical memory in bytes.",
        unit: Some("bytes"),
        value: |s| MetricSampleValue::from(s.memory_free_bytes),
    },
    SystemMetric {
        name: "swap_total_bytes",
        help: "Total swap in bytes.",
        unit: Some("bytes"),
        value: |s| MetricSampleValue::from(s.swap_total_bytes),
    },
    SystemMetric {
        name: "swap_used_bytes",
        help: "Used swap in bytes.",
        unit: Some("bytes"),
        value: |s| MetricSampleValue::from(s.swap_used_bytes),
    },
    SystemMetric {
        name: "cpu_utilization_ratio",
        help: "Average CPU utilisation since the last sample, as a ratio in [0, 1].",
        unit: Some("ratio"),
        value: |s| MetricSampleValue::from(s.cpu_utilization_ratio),
    },
    SystemMetric {
        name: "load1",
        help: "1-minute load average (0 on platforms without load average).",
        unit: None,
        value: |s| MetricSampleValue::from(s.load1),
    },
    SystemMetric {
        name: "load5",
        help: "5-minute load average (0 on platforms without load average).",
        unit: None,
        value: |s| MetricSampleValue::from(s.load5),
    },
    SystemMetric {
        name: "load15",
        help: "15-minute load average (0 on platforms without load average).",
        unit: None,
        value: |s| MetricSampleValue::from(s.load15),
    },
    SystemMetric {
        name: "uptime_seconds",
        help: "Host uptime in seconds.",
        unit: Some("seconds"),
        value: |s| MetricSampleValue::from(s.uptime_seconds),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use metered::Registry;
    use metered_om::{OpenMetricsDocument, OpenMetricsRegistryExt};

    #[test]
    fn system_metrics_render_under_the_system_prefix() {
        let system = SystemMetrics::new();
        let mut registry = Registry::new();
        registry.register(
            metered::entry::metric("system")
                .source(&system)
                .help("Host system telemetry"),
        );
        let text = registry.encode_to_string().unwrap();

        assert!(text.contains("# TYPE system_memory_total_bytes gauge"));
        assert!(text.contains("# UNIT system_memory_total_bytes bytes"));
        assert!(text.contains("# UNIT system_uptime_seconds seconds"));
        let doc = OpenMetricsDocument::parse(&text).unwrap();
        let total = doc.sample("system_memory_total_bytes").unwrap();
        assert!(
            total.value.parse::<u64>().unwrap() > 0,
            "total memory should be positive"
        );
    }

    #[cfg(feature = "tokio")]
    #[tokio::test(flavor = "multi_thread")]
    async fn background_sampler_count_follows_the_handle_lifetime() {
        let system = std::sync::Arc::new(SystemMetrics::new());

        assert_eq!(system.live_samplers.load(Ordering::Relaxed), 0);
        let sampler = std::sync::Arc::clone(&system).spawn(std::time::Duration::from_secs(60));
        assert_eq!(
            system.live_samplers.load(Ordering::Relaxed),
            1,
            "collect reads the cache while the sampler handle lives"
        );

        drop(sampler);
        assert_eq!(
            system.live_samplers.load(Ordering::Relaxed),
            0,
            "dropping the sampler returns collect to inline sampling"
        );

        // The scrape path still works (inline again) after the sampler is gone.
        let mut values = MetricValues::new();
        system.collect("system", &[], &mut values);
        assert!(!values.samples().is_empty());
    }

    #[cfg(feature = "tokio")]
    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_one_of_two_samplers_keeps_the_cached_read_path() {
        let system = std::sync::Arc::new(SystemMetrics::new());

        let first = std::sync::Arc::clone(&system).spawn(std::time::Duration::from_secs(60));
        let second = std::sync::Arc::clone(&system).spawn(std::time::Duration::from_secs(60));

        // With a shared boolean this drop would have flipped the scrape back to
        // inline sampling while `second` still runs.
        drop(first);
        assert_eq!(
            system.live_samplers.load(Ordering::Relaxed),
            1,
            "the surviving sampler keeps the cached read path"
        );

        drop(second);
        assert_eq!(system.live_samplers.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn schema_declares_every_system_family() {
        let system = SystemMetrics::new();
        let mut schema = MetricSchema::new();
        system.describe("system", &[], &mut schema);

        for metric in SYSTEM_METRICS {
            let full = join_name("system", metric.name);
            assert!(
                schema.family(&full).is_some(),
                "missing declared family {full}"
            );
        }
        assert_eq!(schema.families().len(), SYSTEM_METRICS.len());
    }
}
