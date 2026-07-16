//! RPC: a tiny transport `server` and the `middleware` metrics layer wrapped
//! around it.
//!
//! These are two separate concerns, so they live in two files: `server` handles
//! requests by delegating to the order service, while `middleware` turns each
//! call's `rpc.server` span into metrics. They share only [`RpcStatus`] -- the
//! protocol status the server returns and the middleware classifies onto the span.

mod middleware;
mod server;

pub use middleware::RpcMetricsLayer;
pub use server::{CreateOrderRequest, RpcServer};

use std::fmt;

/// The RPC response status, shared by the server (it returns one per call) and
/// the middleware (it classifies one onto the `rpc.server` span at close).
///
/// It is also the typed `rpc_status` label: `Display` writes it to the span and
/// the metric, and `FromFieldValue` reads it back when the adapter rebuilds the
/// label key from a closed span (a value that does not parse is a counted,
/// skipped close -- never a silently defaulted label).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum RpcStatus {
    // `Default` is the last-resort label value when a span closes without a
    // status recorded (it also matches the `default = "OK"` on `rpc_status`).
    #[default]
    Ok,
    InvalidArgument,
    Internal,
}

impl std::str::FromStr for RpcStatus {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, ()> {
        match value {
            "OK" => Ok(RpcStatus::Ok),
            "INVALID_ARGUMENT" => Ok(RpcStatus::InvalidArgument),
            "INTERNAL" => Ok(RpcStatus::Internal),
            _ => Err(()),
        }
    }
}

// The typed capture seam: the adapter rebuilds `rpc_status` from the closed
// span's captured value. An unrecognized status is a contract violation the
// layer skips and counts, not a silently defaulted label.
impl metered_tracing::FromFieldValue for RpcStatus {
    fn from_text(text: &str) -> Result<Self, metered_tracing::FieldValueError> {
        text.parse().map_err(|()| metered_tracing::FieldValueError {
            expected: "RpcStatus",
        })
    }
}

impl RpcStatus {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            RpcStatus::Ok => "OK",
            RpcStatus::InvalidArgument => "INVALID_ARGUMENT",
            RpcStatus::Internal => "INTERNAL",
        }
    }

    pub(crate) fn otel_status_code(self) -> &'static str {
        match self {
            RpcStatus::Ok => "OK",
            RpcStatus::InvalidArgument | RpcStatus::Internal => "ERROR",
        }
    }

    pub(crate) fn error_type(self) -> &'static str {
        match self {
            RpcStatus::Ok => "none",
            RpcStatus::InvalidArgument => "validation",
            RpcStatus::Internal => "internal",
        }
    }
}

// The `rpc_status` label is read from `RpcStatus` via `Display`, so the typed
// enum flows straight into the span field and the metric label.
impl fmt::Display for RpcStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
