use metered::MetricTreeView;
use metered_om::OpenMetricsViewExt;
use metered_semantic::{metered, Elapsed, HitCount};
use std::sync::atomic::AtomicU64;

struct App {
    requests: AtomicU64,
    processed: AtomicU64,
    api: Api,
    enabled: bool,
    subservice: Subservice,
}

struct Subservice {
    jobs: AtomicU64,
}

#[derive(Default)]
struct Api {
    metrics: ApiMetrics,
}

#[metered(registry = ApiMetrics)]
impl Api {
    #[measure([HitCount, Elapsed])]
    fn handle(&self) {}
}

#[test]
fn metric_tree_view_borrows_metrics_from_a_runtime_context() {
    let app = App {
        requests: AtomicU64::new(0),
        processed: AtomicU64::new(0),
        api: Api::default(),
        enabled: true,
        subservice: Subservice {
            jobs: AtomicU64::new(0),
        },
    };
    metered::Counter::incr(&app.requests);
    metered::Counter::incr(&app.processed);
    metered::Counter::incr(&app.subservice.jobs);
    app.api.handle();

    let mut subservice_view = MetricTreeView::new();
    subservice_view.register(
        metered::entry::metric("jobs")
            .select(|subservice: &Subservice| &subservice.jobs)
            .help("Subservice jobs"),
    );

    let mut view = MetricTreeView::with_prefix("app");
    view.label("service", "demo");
    view.register(
        metered::entry::metric("requests")
            .select(|app: &App| &app.requests)
            .help("Total requests"),
    );
    view.register(
        metered::entry::metric("api")
            .select(|app: &App| &app.api.metrics)
            .help("Generated API metrics"),
    );
    view.register(
        metered::entry::tree("subservice")
            .select(|app: &App| &app.subservice)
            .view(subservice_view),
    );
    view.register(
        metered::entry::counter_value("processed")
            .read(|app: &App| metered::CounterSource::get(&app.processed))
            .help("Processed items"),
    );
    view.register(
        metered::entry::gauge_value("enabled")
            .read(|app: &App| app.enabled as i64)
            .help("Whether the app is enabled"),
    );

    let text = view.encode_to_string(&app).unwrap();
    assert!(text.contains("app_requests_total{service=\"demo\"} 1"));
    assert!(text.contains("app_api_handle_hit_count_total{service=\"demo\"} 1"));
    assert!(text.contains("app_subservice_jobs_total{service=\"demo\"} 1"));
    assert!(text.contains("app_processed_total{service=\"demo\"} 1"));
    assert!(text.contains("app_enabled{service=\"demo\"} 1"));

    let schema = view.schema(&app);
    assert!(schema.family("app_requests").is_some());
    assert!(schema.family("app_api_handle_elapsed").is_some());
    assert!(schema.family("app_subservice_jobs").is_some());
    assert!(schema.family("app_processed").is_some());
    assert!(schema.family("app_enabled").is_some());

    let values = view.values(&app);
    assert!(values.samples().iter().any(
        |sample| sample.name == "app_subservice_jobs_total" && sample.value.to_string() == "1"
    ));
    assert!(values
        .samples()
        .iter()
        .any(|sample| sample.name == "app_enabled" && sample.value.to_string() == "1"));
}
