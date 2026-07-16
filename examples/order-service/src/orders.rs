//! The order domain: business logic, business counters, and an in-memory cache.

use crate::db::{Db, DbError};
use metered::{Family, LabelSet, MetricTree, MetricTreeView, MetricsView};
use metered_tracing::{SpanMetric, SpanMetricsSource, SpanRecorder};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct OrderService {
    /// The `orders.create_order` operation metric, owned here next to the span
    /// that drives it. Exported under `orders_create`, also handed to the
    /// routing layer; `metered` only reads it.
    create: SpanMetric,
    pub business: BusinessMetrics,
    pub cache: OrderCache,
    next_order_id: AtomicU64,
}

/// The service owns the span metrics for the `orders.create_order` operation
/// spans it emits, alongside its directly-owned [`BusinessMetrics`].
impl SpanMetricsSource for OrderService {
    fn span_recorders(self: &Arc<Self>) -> Vec<Box<dyn SpanRecorder>> {
        vec![Box::new(self.create.clone())]
    }
}

impl OrderService {
    pub fn demo() -> Self {
        OrderService {
            // Span attributes follow OTel semconv (dotted); the `SpanMetric`
            // translates them to OpenMetrics labels (snake): `order.category`
            // -> `category`.
            create: SpanMetric::for_span("orders.create_order")
                .help("Order creation operations")
                .label("category", "order.category")
                .label("channel", "order.channel")
                .build(),
            business: BusinessMetrics::default(),
            cache: OrderCache::default(),
            next_order_id: AtomicU64::new(1),
        }
    }

    pub(crate) fn create_order(&self, db: &Db, draft: OrderDraft) -> Result<(), OrderError> {
        let category = Category::parse(&draft.category).ok_or(OrderError::InvalidCategory)?;
        let channel = SalesChannel::parse(&draft.channel).ok_or(OrderError::InvalidChannel)?;
        if draft.quantity == 0 {
            return Err(OrderError::InvalidQuantity);
        }

        self.persist_created_order(db, category, channel, draft.quantity)
    }

    // The `orders.create_order` span drives the `orders_create` SpanMetric
    // (count + duration labeled by category/channel); the business counter below
    // is owned and incremented directly. The local `order_id` becomes the
    // duration exemplar -- a non-distributed-tracing exemplar that points each
    // latency bucket at the exact order behind it.
    #[tracing::instrument(
        name = "orders.create_order",
        skip_all,
        fields(
            order.category = category.as_str(),
            order.channel = channel.as_str(),
            order.quantity = quantity,
            order.id = tracing::field::Empty,
        )
    )]
    fn persist_created_order(
        &self,
        db: &Db,
        category: Category,
        channel: SalesChannel,
        quantity: u64,
    ) -> Result<(), OrderError> {
        let order_id = self.next_order_id.fetch_add(1, Ordering::Relaxed);
        tracing::Span::current().record("order.id", order_id);

        db.insert_order(quantity)?;
        self.business.record_order_created(category, channel);
        self.cache.insert_created_order();
        Ok(())
    }
}

pub(crate) struct OrderDraft {
    pub category: String,
    pub channel: String,
    pub quantity: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum OrderError {
    InvalidCategory,
    InvalidChannel,
    InvalidQuantity,
    Db,
}

impl From<DbError> for OrderError {
    fn from(_: DbError) -> Self {
        OrderError::Db
    }
}

#[derive(Default, MetricTree)]
pub struct BusinessMetrics {
    #[metric(help = "Orders created, by category and channel")]
    created: Family<OrderLabels, AtomicU64>,
}

impl BusinessMetrics {
    fn record_order_created(&self, category: Category, channel: SalesChannel) {
        let labels = OrderLabels { category, channel };
        self.created.with(&labels, metered::Counter::incr);
    }
}

/// A realistic in-memory order cache: it owns its state and is mutated through
/// its own methods. `metered` only *reads* its size as a gauge.
pub struct OrderCache {
    next_id: AtomicU64,
    orders: Mutex<HashMap<u64, ()>>,
}

impl Default for OrderCache {
    fn default() -> Self {
        OrderCache {
            next_id: AtomicU64::new(1),
            orders: Mutex::new(HashMap::new()),
        }
    }
}

impl OrderCache {
    fn insert_created_order(&self) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.orders.lock().insert(id, ());
    }

    fn len(&self) -> usize {
        self.orders.lock().len()
    }
}

impl MetricsView for OrderService {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        let mut view = MetricTreeView::new();
        // The span-derived operation metric, mounted where it belongs (under the
        // `orders` field in the app, `create` yields `..._orders_create_*`). Its
        // help/unit ride along from the `SpanMetric` itself.
        view.register(metered::entry::metric("create").select(|svc: &OrderService| &svc.create));
        // Pure business metrics: flatten so the counter is `orders_created_total`,
        // not `orders_business_orders_created_total`.
        view.flatten(|svc: &OrderService| &svc.business);
        view.register(
            metered::entry::gauge_value("cache_entries")
                .read(|svc: &OrderService| svc.cache.len())
                .help("Orders held in the in-memory cache"),
        );
        view
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Category(&'static str);

impl Category {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "books" => Some(Category("books")),
            "home_goods" => Some(Category("home_goods")),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct SalesChannel(&'static str);

impl SalesChannel {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "web" => Some(SalesChannel("web")),
            "mobile" => Some(SalesChannel("mobile")),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for SalesChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// `#[derive(LabelSet)]` turns each field into a label (name = field name, value
// via `Display`), replacing a hand-written `LabelSet` impl.
#[derive(Clone, Debug, Eq, Hash, PartialEq, LabelSet)]
struct OrderLabels {
    category: Category,
    channel: SalesChannel,
}
