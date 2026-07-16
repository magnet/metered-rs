use metered::{BucketHistogram, MetricTree};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[derive(Default, MetricTree)]
struct PoolMetrics {
    // `gauge`/`counter` choose the leaf type; `help`/`unit` annotate the family.
    // `AtomicU64` defaults to a counter, so `gauge` flips `idle` to a gauge.
    #[metric(counter, help = "Connections acquired")]
    acquired: AtomicU64,
    #[metric(gauge, help = "Idle connections")]
    idle: AtomicU64,
    #[metric(unit = "seconds", help = "Checkout wait")]
    wait_seconds: BucketHistogram,
}

#[derive(Default, MetricTree)]
#[metrics(prefix = "sample", label(service = "orders"))]
struct AppMetrics {
    #[metric(counter, rename = "requests", help = "Requests handled")]
    request_count: AtomicU64,
    // `AtomicI64` is already a gauge; no override needed.
    #[metric(help = "Worker queue depth")]
    queue_depth: AtomicI64,
    #[metrics(flatten)]
    pool: PoolMetrics,
}

// Plain atomic updates: the derive reads the same fields at collect time.
fn record(metrics: &AppMetrics) {
    metrics.request_count.fetch_add(3, Ordering::Relaxed);
    metrics.queue_depth.store(7, Ordering::Relaxed);
    metrics.pool.acquired.fetch_add(1, Ordering::Relaxed);
    metrics.pool.idle.store(2, Ordering::Relaxed);
    metrics.pool.wait_seconds.observe(0.004);
}
