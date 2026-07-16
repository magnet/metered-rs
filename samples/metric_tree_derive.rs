use metered::{BucketHistogram, Counter, Gauge, MetricTree};
use std::sync::atomic::{AtomicI64, AtomicU64};

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

fn record(metrics: &AppMetrics) {
    Counter::incr_by(&metrics.request_count, 3);
    Gauge::set(&metrics.queue_depth, 7);
    Counter::incr(&metrics.pool.acquired);
    Gauge::set(&metrics.pool.idle, 2);
    metrics.pool.wait_seconds.observe(0.004);
}
