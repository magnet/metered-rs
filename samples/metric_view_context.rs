use metered::entry::{counter, gauge, info, tree};
use metered::{InfoMetric, MetricTreeView, Unit};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct App {
    build: InfoMetric,
    requests: AtomicU64,
    queue_depth: AtomicU64,
    worker: Worker,
}

pub struct Worker {
    jobs: AtomicU64,
}

// Typed entry builders are the one registration path: each entry carries its
// selector plus its own `help`/`unit`, so metadata can never attach to the
// wrong entry. The selector closure needs the `&Worker` arg annotation (a
// closure returning `&T` always does).
fn worker_view() -> MetricTreeView<'static, Worker> {
    let mut view = MetricTreeView::new();
    view.register(
        counter("jobs")
            .select(|worker: &Worker| &worker.jobs)
            .help("Worker jobs"),
    );
    view
}

pub fn app_view() -> MetricTreeView<'static, App> {
    let mut view = MetricTreeView::with_prefix("sample");
    view.label("service", "orders");
    view.register(
        info("build")
            .select(|app: &App| &app.build)
            .help("Build information"),
    );
    view.register(
        counter("requests")
            .select(|app: &App| &app.requests)
            .help("Requests"),
    );
    view.register(
        gauge("queue_depth")
            .select(|app: &App| &app.queue_depth)
            .help("Queued work")
            .unit(Unit::Items),
    );
    view.register(
        tree("worker")
            .select(|app: &App| &app.worker)
            .view(worker_view()),
    );
    view
}

pub fn record(app: &App) {
    app.requests.fetch_add(1, Ordering::Relaxed);
    app.queue_depth.store(4, Ordering::Relaxed);
    app.worker.jobs.fetch_add(1, Ordering::Relaxed);
}
