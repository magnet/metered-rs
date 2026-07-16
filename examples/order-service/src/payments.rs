//! Payment rails: the dynamic fleet an order service settles payments across.
//!
//! This is the "sub-service" shape that motivates
//! [`each`](metered::MetricTreeView::each):
//!
//! - The set of rails is **dynamic** -- a rail (a card network, a wallet, a
//!   buy-now-pay-later provider) is onboarded at runtime ([`PaymentRails::register`]),
//!   not known at startup.
//! - A [`PaymentRail`] is split into its **business logic** (its real config --
//!   the endpoint it settles against) and its **metrics**. `in_flight`,
//!   `settlements`, and `failures` are all pure metrics (an in-progress gauge and
//!   two counters) living in `RailMetrics`. The rail's view declares them as
//!   typed entries, so the group's schema is known without a live rail: an
//!   `each` group advertises its families (with the `rail` label) even before
//!   the first rail is onboarded.
//!
//! [`PaymentRails::metrics_view`] walks the live map at collect time and stamps
//! the rail name on every series via `each`, e.g.
//! `payments_settlements_total{rail="card"}`. As a service-specific metric this
//! is namespaced under the app's `order_service_` prefix at composition time, so
//! the scraped name is `order_service_payments_settlements_total{rail="card"}`.

use metered::entry::{counter, gauge};
use metered::{MetricTreeView, MetricsView};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct PaymentRails {
    inner: RwLock<HashMap<String, PaymentRail>>,
}

pub struct PaymentRail {
    /// Business logic: where this rail settles payments.
    endpoint: String,
    /// Metrics: pure observability for the rail.
    metrics: RailMetrics,
}

/// Pure metrics for a rail. The rail mutates these with plain atomic ops;
/// `metered` only reads them, through the typed entries the rail's view
/// declares below.
#[derive(Default)]
struct RailMetrics {
    in_flight: AtomicU64,
    settlements: AtomicU64,
    failures: AtomicU64,
}

impl RailMetrics {
    fn started(&self) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
    }

    fn finished(&self, ok: bool) {
        self.in_flight.fetch_sub(1, Ordering::Relaxed);
        self.settlements.fetch_add(1, Ordering::Relaxed);
        if !ok {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl PaymentRails {
    /// Starts with the rails enabled at boot; more can be onboarded later.
    pub fn demo() -> Self {
        let mut inner = HashMap::new();
        for name in ["card", "wallet"] {
            inner.insert(name.to_owned(), PaymentRail::new(name));
        }
        PaymentRails {
            inner: RwLock::new(inner),
        }
    }

    /// Onboards a rail discovered at runtime (a new provider, a config reload).
    /// Its metrics appear on the next scrape -- nothing else to wire.
    pub fn register(&self, name: &str) {
        self.inner
            .write()
            .entry(name.to_owned())
            .or_insert_with(|| PaymentRail::new(name));
    }

    /// Settles a payment over `name` by delegating to the rail's business logic.
    pub fn settle(&self, name: &str, ok: bool) {
        let rails = self.inner.read();
        if let Some(rail) = rails.get(name) {
            rail.settle(ok);
        }
    }
}

impl PaymentRail {
    fn new(name: &str) -> Self {
        PaymentRail {
            endpoint: format!("https://{name}.rail.internal/settle"),
            metrics: RailMetrics::default(),
        }
    }

    /// Business logic: route a settlement to this rail's endpoint, bracketing it
    /// with the in-flight gauge and recording the outcome.
    fn settle(&self, ok: bool) {
        self.metrics.started();
        // A real rail POSTs the settlement to `self.endpoint` here.
        tracing::debug!(endpoint = %self.endpoint, ok, "settling payment");
        self.metrics.finished(ok);
    }
}

impl MetricsView for PaymentRails {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        let mut view = MetricTreeView::new();
        // The closure owns the lock scope and emits one (name, rail) at a time;
        // `each` stamps the `rail` label on each rail's series.
        view.each(
            "rail",
            PaymentRail::metrics_view(),
            |rails: &PaymentRails, out| {
                for (name, rail) in rails.inner.read().iter() {
                    out.emit(name, rail);
                }
            },
        );
        view
    }
}

impl MetricsView for PaymentRail {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        let mut view = MetricTreeView::new();
        // Typed entries declare each family's shape statically, so the `each`
        // group's schema does not depend on any rail being live. The
        // business-logic `endpoint` is not a metric, so it isn't here.
        view.register(
            gauge("in_flight")
                .select(|rail: &PaymentRail| &rail.metrics.in_flight)
                .help("Settlements in flight on the rail"),
        );
        view.register(
            counter("settlements")
                .select(|rail: &PaymentRail| &rail.metrics.settlements)
                .help("Payments settled over the rail"),
        );
        view.register(
            counter("failures")
                .select(|rail: &PaymentRail| &rail.metrics.failures)
                .help("Failed settlements on the rail"),
        );
        view
    }
}
