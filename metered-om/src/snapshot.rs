//! Rendered OpenMetrics snapshot cache.
//!
//! The cache owns a single rendered snapshot plus a reusable scratch
//! [`BytesMut`](bytes::BytesMut). It has no statics and spawns no task: callers
//! drive [`SnapshotCache::refresh`] from their own maintenance loop.

use crate::{HistogramProfile, OpenMetricsEncoder};
use bytes::{BufMut, Bytes, BytesMut};
use metered::{MetricSchema, MetricTree, MetricValues, Registry, SinkError};
use parking_lot::Mutex;
use std::fmt::{self, Write};
use std::time::Instant;

const DEFAULT_CAPACITY: usize = 8 * 1024;

/// A rendered OpenMetrics snapshot.
#[derive(Clone, Debug)]
pub struct Snapshot {
    created_at: Instant,
    body: Bytes,
}

impl Snapshot {
    /// When this snapshot was rendered. The cache imposes no staleness window
    /// of its own -- [`SnapshotCache::snapshot`] serves whatever was rendered
    /// last -- so a caller that must bound staleness (e.g. before serving the
    /// body over HTTP) checks this against its own budget.
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Snapshot body bytes.
    pub fn body(&self) -> Bytes {
        self.body.clone()
    }

    /// Snapshot body length.
    pub fn len(&self) -> usize {
        self.body.len()
    }

    /// Returns `true` when the snapshot body is empty.
    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }
}

/// Configuration for [`SnapshotCache`].
///
/// `#[non_exhaustive]`: construct via [`SnapshotConfig::default`] and assign
/// the fields to override; further knobs may be added without a breaking
/// change.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct SnapshotConfig {
    /// Histogram rendering profile.
    pub histogram_profile: HistogramProfile,
    /// Whether refresh drives the target's housekeeping before sampling.
    /// Note this only controls the cache's own explicit housekeep pass: a
    /// target whose plain `values()` housekeeps internally (e.g. a
    /// [`Registry`] with its default scrape-time maintenance) still does so
    /// when this is `false`.
    pub housekeep_on_refresh: bool,
    /// Capacity multiplier numerator for the next scratch buffer.
    pub growth_numerator: usize,
    /// Capacity multiplier denominator for the next scratch buffer.
    pub growth_denominator: usize,
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        SnapshotConfig {
            histogram_profile: HistogramProfile::Le,
            housekeep_on_refresh: true,
            // Keep room for the currently cached snapshot and one next snapshot
            // with moderate growth. This keeps allocations stable across
            // refreshes without holding two mutable buffers.
            growth_numerator: 5,
            growth_denominator: 2,
        }
    }
}

/// Something that can render an OpenMetrics snapshot.
pub trait SnapshotTarget {
    /// Performs off-hot-path upkeep before rendering, if any.
    fn housekeep(&self) {}
    /// Describes the families.
    fn schema(&self) -> MetricSchema;
    /// Samples values.
    fn values(&self) -> MetricValues;
    /// Samples values after [`housekeep`](SnapshotTarget::housekeep) has already
    /// been driven for this refresh.
    fn values_after_housekeep(&self) -> MetricValues {
        self.values()
    }
}

/// Snapshot target backed by a [`MetricTree`].
#[derive(Clone, Debug)]
pub struct MetricTreeSnapshot<T> {
    tree: T,
}

impl<T> MetricTreeSnapshot<T> {
    /// Wraps `tree`.
    pub fn new(tree: T) -> Self {
        MetricTreeSnapshot { tree }
    }

    /// Returns the wrapped tree.
    pub fn tree(&self) -> &T {
        &self.tree
    }
}

impl<T: MetricTree> SnapshotTarget for MetricTreeSnapshot<T> {
    fn housekeep(&self) {
        self.tree.housekeep();
    }

    fn schema(&self) -> MetricSchema {
        let mut schema = MetricSchema::new();
        self.tree.describe("", &[], &mut schema);
        schema
    }

    fn values(&self) -> MetricValues {
        let mut values = MetricValues::new();
        self.tree.collect("", &[], &mut values);
        values
    }
}

/// Snapshot target backed by a [`Registry`].
#[derive(Clone, Copy)]
pub struct RegistrySnapshot<'a> {
    registry: &'a Registry<'a>,
}

impl<'a> RegistrySnapshot<'a> {
    /// Wraps `registry`.
    pub fn new(registry: &'a Registry<'a>) -> Self {
        RegistrySnapshot { registry }
    }
}

impl SnapshotTarget for RegistrySnapshot<'_> {
    fn housekeep(&self) {
        self.registry.housekeep();
    }

    fn schema(&self) -> MetricSchema {
        self.registry.schema()
    }

    fn values(&self) -> MetricValues {
        self.registry.values()
    }

    fn values_after_housekeep(&self) -> MetricValues {
        self.registry.values_without_housekeep()
    }
}

#[derive(Debug)]
struct CacheState {
    current: Option<Snapshot>,
    scratch: BytesMut,
}

/// A cache of rendered OpenMetrics snapshots.
///
/// The cache never refreshes on its own and imposes no staleness window:
/// [`snapshot`](SnapshotCache::snapshot) serves the last rendered body however
/// old it is. Callers that must bound staleness compare
/// [`Snapshot::created_at`] against their own budget before serving.
#[derive(Debug)]
pub struct SnapshotCache<T> {
    target: T,
    config: SnapshotConfig,
    /// Serializes whole refreshes (housekeep + collect + render + publish);
    /// see [`SnapshotCache::refresh`].
    refresh: Mutex<()>,
    state: Mutex<CacheState>,
}

impl<T> SnapshotCache<T> {
    /// Creates a cache over `target`.
    pub fn new(target: T) -> Self {
        SnapshotCache::with_config(target, SnapshotConfig::default())
    }

    /// Creates a cache over `target` with explicit config.
    pub fn with_config(target: T, config: SnapshotConfig) -> Self {
        SnapshotCache {
            target,
            config,
            refresh: Mutex::new(()),
            state: Mutex::new(CacheState {
                current: None,
                scratch: BytesMut::with_capacity(DEFAULT_CAPACITY),
            }),
        }
    }

    /// Returns the current cached snapshot, if one has been rendered.
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.state.lock().current.clone()
    }
}

impl<T: SnapshotTarget> SnapshotCache<T> {
    /// Refreshes the cached snapshot.
    ///
    /// This method is synchronous and performs all collection/rendering work on
    /// the caller's thread. Drive it from an external maintenance task or scrape
    /// coordinator; the cache itself spawns no background work. Concurrent
    /// callers are safe but serialized (see below), so prefer a single driver.
    pub fn refresh(&self) -> Result<Snapshot, SinkError> {
        // Serialize the *entire* refresh, not just the publish. Housekeeping
        // and value collection run outside the `state` lock (so `snapshot()`
        // readers never wait on a render); if two refreshes could interleave,
        // the one that collected *earlier* could publish *later*, caching
        // older values under a newer `created_at` -- counters would appear to
        // go backwards between scrapes served from the cache -- and both would
        // drive `housekeep`, double-swapping per-scrape state such as exemplar
        // windows. Holding this guard across collect + render + publish makes
        // publish order equal collection order and keeps housekeeping
        // once-per-refresh, while readers only ever contend on the brief
        // `state` lock.
        let _refresh = self.refresh.lock();
        let maintained = self.config.housekeep_on_refresh;
        if maintained {
            self.target.housekeep();
        }

        let schema = self.target.schema();
        let values = if maintained {
            self.target.values_after_housekeep()
        } else {
            self.target.values()
        };

        let mut state = self.state.lock();
        let mut scratch = std::mem::take(&mut state.scratch);
        scratch.clear();
        encode_document(
            &mut scratch,
            &schema,
            &values,
            self.config.histogram_profile,
        )?;

        let body = scratch.freeze();
        let old = state.current.replace(Snapshot {
            created_at: Instant::now(),
            body: body.clone(),
        });

        state.scratch = reusable_buffer(old, body.len(), self.config);
        Ok(state
            .current
            .as_ref()
            .expect("snapshot just stored")
            .clone())
    }
}

fn encode_document(
    out: &mut BytesMut,
    schema: &MetricSchema,
    values: &MetricValues,
    profile: HistogramProfile,
) -> Result<(), SinkError> {
    let mut writer = BytesMutWriter(out);
    let mut encoder = OpenMetricsEncoder::new(&mut writer).histogram_profile(profile);
    encoder.encode_document(schema, values)?;
    encoder.finish()
}

fn reusable_buffer(old: Option<Snapshot>, last_len: usize, config: SnapshotConfig) -> BytesMut {
    if let Some(snapshot) = old {
        if let Ok(mut bytes) = snapshot.body.try_into_mut() {
            bytes.clear();
            // The buffer is empty, so `reserve(n)` guarantees capacity >= n
            // (and is a no-op when the capacity already suffices).
            bytes.reserve(padded_capacity(last_len, config));
            return bytes;
        }
    }
    BytesMut::with_capacity(padded_capacity(last_len, config).max(DEFAULT_CAPACITY))
}

fn padded_capacity(last_len: usize, config: SnapshotConfig) -> usize {
    let denominator = config.growth_denominator.max(1);
    last_len.saturating_mul(config.growth_numerator.max(1)) / denominator
}

struct BytesMutWriter<'a>(&'a mut BytesMut);

impl Write for BytesMutWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0.put_slice(s.as_bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metered::{Counter, Metric, MetricType, MetricValues, Registry};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    #[test]
    fn cache_refreshes_and_reuses_unique_previous_snapshot_buffer() {
        let requests = AtomicU64::new(0);
        let mut registry = Registry::new();
        registry.register(
            metered::entry::metric("requests")
                .source(&requests)
                .help("Requests"),
        );
        let cache = SnapshotCache::new(RegistrySnapshot::new(&registry));

        requests.incr();
        let first = cache.refresh().unwrap();
        assert!(
            std::str::from_utf8(&first.body())
                .unwrap()
                .contains("requests_total 1")
        );

        // Drop the clone before refreshing so the old Bytes can be reclaimed as
        // scratch.
        drop(first);
        requests.incr();
        let second = cache.refresh().unwrap();
        assert!(
            std::str::from_utf8(&second.body())
                .unwrap()
                .contains("requests_total 2")
        );
        assert!(second.len() <= cache.state.lock().scratch.capacity());
    }

    #[test]
    fn cache_snapshot_returns_last_render_without_refreshing() {
        let requests = AtomicU64::new(0);
        let mut registry = Registry::new();
        registry.register(
            metered::entry::metric("requests")
                .source(&requests)
                .help("Requests"),
        );
        let cache = SnapshotCache::new(RegistrySnapshot::new(&registry));

        requests.incr();
        let rendered = cache.refresh().unwrap();
        requests.incr();
        assert_eq!(cache.snapshot().unwrap().body(), rendered.body());
    }

    struct HousekeepMetric {
        runs: AtomicUsize,
    }

    impl Metric for HousekeepMetric {
        fn metric_type(&self) -> MetricType {
            MetricType::Gauge
        }

        fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
            values.gauge(name, labels, self.runs.load(Ordering::Relaxed));
        }

        fn housekeep(&self) {
            self.runs.fetch_add(1, Ordering::Relaxed);
        }

        fn needs_housekeep(&self) -> bool {
            true
        }
    }

    #[test]
    fn concurrent_refreshes_never_overlap_collection() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        // Detects overlapping value collection. With whole refreshes
        // serialized, `values` can never run concurrently, which is the
        // property the ordering fix relies on: publish order equals collect
        // order, so a refresh cannot cache older values under a newer
        // timestamp. (The out-of-order publish itself needs a precise
        // interleaving we cannot force deterministically without injection
        // hooks, so the test asserts the serialization invariant instead.)
        struct OverlapDetector {
            collecting: AtomicBool,
            overlapped: Arc<AtomicBool>,
        }

        impl SnapshotTarget for OverlapDetector {
            fn schema(&self) -> MetricSchema {
                MetricSchema::new()
            }

            fn values(&self) -> MetricValues {
                if self.collecting.swap(true, Ordering::SeqCst) {
                    self.overlapped.store(true, Ordering::SeqCst);
                }
                std::thread::yield_now();
                self.collecting.store(false, Ordering::SeqCst);
                MetricValues::new()
            }
        }

        let overlapped = Arc::new(AtomicBool::new(false));
        let cache = Arc::new(SnapshotCache::new(OverlapDetector {
            collecting: AtomicBool::new(false),
            overlapped: Arc::clone(&overlapped),
        }));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let cache = Arc::clone(&cache);
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        cache.refresh().unwrap();
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(
            !overlapped.load(Ordering::SeqCst),
            "concurrent refreshes must be serialized end to end"
        );
    }

    #[test]
    fn registry_snapshot_cache_runs_housekeep_once_per_refresh() {
        let metric = HousekeepMetric {
            runs: AtomicUsize::new(0),
        };
        let mut registry = Registry::new();
        registry.register(metered::entry::metric("housekeep_runs").source(&metric));
        let cache = SnapshotCache::new(RegistrySnapshot::new(&registry));

        let rendered = cache.refresh().unwrap();
        let bytes = rendered.body();
        let body = std::str::from_utf8(&bytes).unwrap();

        assert!(
            body.contains("housekeep_runs 1"),
            "refresh should maintain once before sampling, got:\n{body}"
        );
        assert_eq!(metric.runs.load(Ordering::Relaxed), 1);
    }
}
