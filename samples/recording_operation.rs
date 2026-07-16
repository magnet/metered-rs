// Requires:
//
// metered-semantic = { version = "0.10", features = ["recording"] }
//
// Use explicit operations for code paths that do not naturally map to tracing
// spans. For span-oriented code, prefer `metered-tracing`.

use metered_semantic::{measure, recording::Operation};

pub fn load_order(operation: &Operation, id: u64) -> Result<String, &'static str> {
    measure!(operation, {
        if id == 0 {
            Err("missing order id")
        } else {
            Ok(format!("order-{id}"))
        }
    })
}
