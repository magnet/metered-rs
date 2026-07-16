//! The application container and the one place metrics are composed for export.

use crate::db::Db;
use crate::jobs::JobRunner;
use crate::orders::OrderService;
use crate::payments::PaymentRails;
use crate::rpc::{CreateOrderRequest, RpcMetricsLayer, RpcServer};
use metered::{Info, Labels, MetricSchema, MetricTree, MetricValues, MetricsView};
use metered_tracing::TracingMetrics;
use std::sync::Arc;
use tracing_subscriber::prelude::*;

/// A component shared between the metrics export tree and the tracing layer.
///
/// [`SpanMetricsSource`](metered_tracing::SpanMetricsSource) and the
/// [`SpanDurations::on`](metered_tracing::SpanDurations::on) projection both take
/// `self: &Arc<Self>`: a component that is *both* exported (through its
/// [`MetricsView`]) and a source of span recorders must therefore live behind a
/// single shared `Arc`. `Shared` is that one handle. It mounts in a
/// `#[derive(MetricTree)]` tree as a `#[metrics(tree)]` field, delegating to the
/// component's `MetricsView`, while `arc` hands the same
/// `&Arc<T>` to the tracing layer -- so the recorders write into the very
/// families the view exports.
pub struct Shared<T>(Arc<T>);

impl<T> Shared<T> {
    fn new(value: T) -> Self {
        Shared(Arc::new(value))
    }

    /// The shared component handle, for the tracing layer's `source` / adapters.
    fn arc(&self) -> &Arc<T> {
        &self.0
    }
}

impl<T> std::ops::Deref for Shared<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: MetricsView> MetricTree for Shared<T> {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        T::metrics_view().describe_prefixed(&*self.0, Some(name), labels, schema);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        T::metrics_view().collect_prefixed(&*self.0, Some(name), labels, values);
    }

    fn housekeep(&self) {
        T::metrics_view().housekeep_entries(&*self.0);
    }
}

/// Service identity, modeled as the service's own type and exported as an
/// OpenMetrics `info` series by implementing [`Info`].
pub struct ServiceIdentity {
    name: &'static str,
    version: &'static str,
    region: &'static str,
}

/// Runtime configuration used by the app, deliberately not exported as metrics:
/// `#[derive(MetricTree)]` is opt-in per field.
pub struct AppConfig {
    pub max_checkout_quantity: u64,
}

impl ServiceIdentity {
    fn demo() -> Self {
        ServiceIdentity {
            name: "order-service",
            version: env!("CARGO_PKG_VERSION"),
            region: "eu-west-1",
        }
    }
}

impl Info for ServiceIdentity {
    fn labels(&self) -> Labels {
        Labels::new([
            ("service_name", self.name),
            ("service_version", self.version),
            ("region", self.region),
        ])
    }
}

/// The standard, cross-service metrics -- the "API spec" every service speaks
/// via OTel semantic conventions (`rpc.*`, `db.client.*`). Their family names are
/// the shared convention, so the producing service is a `service` **label**,
/// letting them aggregate across the fleet:
/// `rpc_server_requests_total{service="order-service"}`.
#[derive(MetricTree)]
#[metrics(label(service = "order-service"))]
pub struct Standard {
    #[metrics(tree)]
    pub rpc: Shared<RpcMetricsLayer>,
    #[metrics(tree)]
    pub db: Shared<Db>,
}

/// The service-specific metrics -- business and internal state only *this*
/// service defines (orders, payments, the job queue). They live in the service's
/// own `order_service_` **namespace prefix**; the prefix is the identity, so they
/// carry no `service` label: `order_service_orders_created_total`.
#[derive(MetricTree)]
#[metrics(prefix = "order_service")]
pub struct Business {
    #[metrics(tree)]
    pub orders: Shared<OrderService>,
    #[metrics(tree)]
    pub jobs: Shared<JobRunner>,
    #[metrics]
    pub payments: PaymentRails,
}

/// The composition root. `App` is the business object; the field annotations
/// define the exposition shape, and the split between the two groups above
/// encodes the naming policy:
///
/// - **Standard / cross-service metrics** ([`Standard`]): shared name + `service`
///   label, so RPC/DB metrics aggregate across services.
/// - **Service-specific metrics** ([`Business`]): namespaced under the
///   `order_service_` prefix, since only this service defines them.
/// - **Service identity** (`service_info`): the top-level `Info` series carrying
///   `service_name` / `service_version` / `region`.
///
/// Both groups are `flatten`ed in, so each group's metrics sit at the document
/// root carrying only its own label/prefix policy. Components still own their
/// metrics; the tracing layer that *records* the span-derived ones is assembled
/// separately in [`App::run_with_tracing`].
#[derive(MetricTree)]
pub struct App {
    #[metrics(info, rename = "service", help = "Service build metadata")]
    pub identity: ServiceIdentity,
    pub config: AppConfig,
    #[metrics(flatten)]
    pub standard: Standard,
    #[metrics(flatten)]
    pub business: Business,
}

impl App {
    pub fn demo() -> Self {
        App {
            identity: ServiceIdentity::demo(),
            config: AppConfig {
                max_checkout_quantity: 4,
            },
            standard: Standard {
                rpc: Shared::new(RpcMetricsLayer::new()),
                db: Shared::new(Db::demo()),
            },
            business: Business {
                orders: Shared::new(OrderService::demo()),
                jobs: Shared::new(JobRunner::demo()),
                payments: PaymentRails::demo(),
            },
        }
    }

    pub fn rpc(&self) -> RpcServer<'_> {
        RpcServer::new(&self.standard.rpc, &self.business.orders, &self.standard.db)
    }

    pub fn jobs(&self) -> &JobRunner {
        &self.business.jobs
    }

    /// Runs `f` with the span-metrics layer installed, so every span the work
    /// opens is turned into a semantic metric.
    ///
    /// The routing layer is assembled here, at the composition root, from the
    /// same component-owned metrics those components export through their views:
    /// `rpc` contributes a stateless [`SpanDurations`](metered_tracing::SpanDurations)
    /// adapter via `.recorder`, the others contribute `SpanMetric`s via `.source`.
    /// The layer only writes.
    pub fn run_with_tracing<R>(&self, f: impl FnOnce() -> R) -> R {
        let telemetry = TracingMetrics::builder()
            .recorder(self.standard.rpc.arc().duration_adapter())
            .source(self.business.orders.arc())
            .source(self.standard.db.arc())
            .source(self.business.jobs.arc())
            .build()
            .with_exemplar_provider(crate::telemetry::exemplar_provider());
        let subscriber = tracing_subscriber::registry().with(telemetry);
        tracing::subscriber::with_default(subscriber, f)
    }
}

pub fn run_demo_workload(app: &App, iterations: usize) {
    for i in 0..iterations {
        let category = if i % 2 == 0 { "books" } else { "home_goods" };
        let channel = if i % 3 == 0 { "web" } else { "mobile" };

        let _ = app.rpc().create_order(CreateOrderRequest {
            category: category.to_owned(),
            channel: channel.to_owned(),
            quantity: (i % 4 + 1) as u64,
        });

        // Settle each order's payment over a rail. A "bnpl" rail is *onboarded
        // at runtime* halfway through and starts taking traffic -- `each` picks
        // it up on the next scrape with no extra wiring. 1 in 10 fails.
        if i == iterations / 2 {
            app.business.payments.register("bnpl");
        }
        let rail = if i >= iterations / 2 && i % 3 == 0 {
            "bnpl"
        } else if i % 2 == 0 {
            "card"
        } else {
            "wallet"
        };
        app.business.payments.settle(rail, i % 10 != 0);

        app.jobs().run_once();
    }
}
