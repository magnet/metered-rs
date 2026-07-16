//! The RPC server: handles requests by delegating to the order service.
//!
//! It owns no metrics. Each call is wrapped by the [`RpcMetricsLayer`] middleware
//! (`metrics_layer.call(method, handler)`), which opens the `rpc.server` span the
//! metrics are derived from. The server just maps requests to the domain and
//! domain errors back to an [`RpcStatus`].

use super::RpcStatus;
use super::middleware::RpcMetricsLayer;
use crate::db::Db;
use crate::orders::{OrderDraft, OrderError, OrderService};

const CREATE_ORDER_METHOD: &str = "CreateOrder";

pub struct CreateOrderRequest {
    pub category: String,
    pub channel: String,
    pub quantity: u64,
}

pub struct RpcServer<'a> {
    metrics_layer: &'a RpcMetricsLayer,
    orders: &'a OrderService,
    db: &'a Db,
}

impl<'a> RpcServer<'a> {
    pub fn new(metrics_layer: &'a RpcMetricsLayer, orders: &'a OrderService, db: &'a Db) -> Self {
        RpcServer {
            metrics_layer,
            orders,
            db,
        }
    }

    pub fn create_order(&self, request: CreateOrderRequest) -> RpcStatus {
        self.metrics_layer.call(CREATE_ORDER_METHOD, || {
            self.orders
                .create_order(
                    self.db,
                    OrderDraft {
                        category: request.category,
                        channel: request.channel,
                        quantity: request.quantity,
                    },
                )
                .map(|_| RpcStatus::Ok)
                .unwrap_or_else(map_order_error)
        })
    }
}

fn map_order_error(error: OrderError) -> RpcStatus {
    match error {
        OrderError::InvalidCategory | OrderError::InvalidChannel | OrderError::InvalidQuantity => {
            RpcStatus::InvalidArgument
        }
        OrderError::Db => RpcStatus::Internal,
    }
}
