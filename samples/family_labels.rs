use metered::{Counter, Family, LabelSet};
use std::sync::atomic::AtomicU64;

#[derive(Clone, PartialEq, Eq, Hash, LabelSet)]
struct HttpLabels {
    // Use a bounded route template like "/orders/:id", never a raw request path
    // containing user IDs or other high-cardinality values.
    route: String,
    status: u16,
}

struct HttpMetrics {
    requests: Family<HttpLabels, AtomicU64>,
}

impl HttpMetrics {
    fn new() -> Self {
        HttpMetrics {
            requests: Family::default(),
        }
    }

    fn record(&self, route: &str, status: u16) {
        self.requests.with(
            &HttpLabels {
                route: route.to_owned(),
                status,
            },
            Counter::incr,
        );
    }
}
