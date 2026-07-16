use metered::entry::{counter, gauge};
use metered::{MetricTreeView, Unit};
use std::sync::atomic::{AtomicU64, Ordering};

struct App {
    requests: AtomicU64,
    queue_depth: AtomicU64,
}

fn metrics() -> MetricTreeView<'static, App> {
    let mut view = MetricTreeView::with_prefix("sample");
    view.register(
        counter("requests")
            .select(|app: &App| &app.requests)
            .help("Requests"),
    );
    view.register(
        gauge("queue_depth")
            .select(|app: &App| &app.queue_depth)
            .help("Queue depth")
            .unit(Unit::Items),
    );
    view
}

// The app updates its own state with plain atomic operations; the view above
// is what reads the same fields as metrics.
fn record(app: &App) {
    app.requests.fetch_add(1, Ordering::Relaxed);
    app.queue_depth.store(3, Ordering::Relaxed);
}
