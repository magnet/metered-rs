use metered::entry::{counter, gauge};
use metered::{Counter, Gauge, MetricTreeView, Unit};
use std::sync::atomic::AtomicU64;

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

fn record(app: &App) {
    Counter::incr(&app.requests);
    Gauge::set(&app.queue_depth, 3);
}
