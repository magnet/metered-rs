//! A fake in-memory database.
//!
//! The connection pool is *real internal state*: plain atomics the DB mutates as
//! it checks connections in and out. You can't (and shouldn't) hang `#[metric]`
//! attributes on operational internals like this. So the DB's metrics are a
//! **view** over those internals, and the view has total control over the wire
//! schema -- it picks each family's name, type (gauge vs counter), and help, and
//! can even synthesize a metric (`pool_utilization`) that no field holds. This is
//! the opposite end from `BusinessMetrics`, which *is* pure metrics and uses
//! `#[derive(MetricTree)]`.
//!
//! Query latency/throughput come from the `db.query` span via a [`SpanMetric`]
//! the DB owns and mounts under `db_client`; the DB never times itself.

use metered::entry::{counter, gauge, gauge_value, metric};
use metered::{MetricTreeView, MetricsView};
use metered_tracing::{SpanMetric, SpanMetricsSource, SpanRecorder};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Db {
    /// The `db.query` metric, owned next to the span that drives it. Mounted
    /// under the `client` segment (OTel's `db.client.*` namespace), not `query`.
    query: SpanMetric,
    pool: ConnectionPool,
    query_errors: AtomicU64,
}

/// Real pool internals: operational state the DB manages, not a metrics struct.
struct ConnectionPool {
    in_use: AtomicU64,
    idle: AtomicU64,
}

impl SpanMetricsSource for Db {
    fn span_recorders(self: &Arc<Self>) -> Vec<Box<dyn SpanRecorder>> {
        vec![Box::new(self.query.clone())]
    }
}

impl Db {
    pub fn demo() -> Self {
        Db {
            query: SpanMetric::for_span("db.query")
                .help("Database queries")
                .label("db_operation", "db.operation")
                .build(),
            pool: ConnectionPool {
                in_use: AtomicU64::new(0),
                idle: AtomicU64::new(4),
            },
            query_errors: AtomicU64::new(0),
        }
    }

    pub(crate) fn insert_order(&self, quantity: u64) -> Result<(), DbError> {
        // The `db.query` span (semconv fields) is what the `db_client` SpanMetric
        // turns into query latency + throughput; we only open it.
        let span = tracing::info_span!(
            "db.query",
            db.system = "in_memory",
            db.operation = "insert_order"
        );
        span.in_scope(|| {
            let restored_idle = self.pool.checkout();
            let result = if quantity == 0 {
                // Domain state: the DB bumps its own counter; the view reads it.
                self.query_errors.fetch_add(1, Ordering::Relaxed);
                Err(DbError::InvalidQuantity)
            } else {
                Ok(())
            };
            self.pool.release(restored_idle);
            result
        })
    }
}

impl ConnectionPool {
    fn checkout(&self) -> bool {
        self.in_use.fetch_add(1, Ordering::Relaxed);
        self.idle
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |idle| {
                idle.checked_sub(1)
            })
            .is_ok()
    }

    fn release(&self, restore_idle: bool) {
        self.in_use
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |in_use| {
                in_use.checked_sub(1)
            })
            .expect("checked-out DB connection must be released once");
        if restore_idle {
            self.idle.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn in_use(&self) -> u64 {
        self.in_use.load(Ordering::Relaxed)
    }

    fn idle(&self) -> u64 {
        self.idle.load(Ordering::Relaxed)
    }
}

/// The DB's metric schema, shaped by hand over the pool's real internals: the
/// view chooses the names, forces gauge (the atomics would default to counters),
/// and synthesizes `pool_utilization`, which no field stores.
impl MetricsView for Db {
    fn metrics_view() -> MetricTreeView<'static, Db> {
        let mut view = MetricTreeView::new();
        // Span-derived query metric. Under the `db` field, `client` yields
        // `..._db_client_*` -- OTel's `db.client.*` namespace, not `db_query`.
        view.register(metric("client").select(|db: &Db| &db.query));
        view.register(
            gauge("pool_in_use")
                .select(|db: &Db| &db.pool.in_use)
                .help("Pool connections currently checked out"),
        );
        view.register(
            gauge("pool_idle")
                .select(|db: &Db| &db.pool.idle)
                .help("Pool connections sitting idle"),
        );
        view.register(
            gauge_value("pool_utilization")
                .read(|db: &Db| {
                    let in_use = db.pool.in_use() as f64;
                    let total = in_use + db.pool.idle() as f64;
                    if total == 0.0 { 0.0 } else { in_use / total }
                })
                .help("Fraction of pool connections in use"),
        );
        view.register(
            counter("query_errors")
                .select(|db: &Db| &db.query_errors)
                .help("DB write errors"),
        );
        view
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum DbError {
    InvalidQuantity,
}
