use metered_om::OpenMetricsExt;
use order_service_demo::app::{run_demo_workload, App};

fn main() {
    let app = App::demo();
    // Install the span-metrics layer only for the workload; the demo's output is
    // the scraped OpenMetrics document, not span logs.
    app.run_with_tracing(|| run_demo_workload(&app, 100));

    println!(
        "{}",
        app.encode_to_string()
            .expect("encode order service demo metrics")
    );
}
