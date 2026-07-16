use metered::{MetricTree, Registry};
use metered_om::OpenMetricsRegistryExt;
use metered_semantic::{measure, ErrorCount, HitCount};

mod baz;
use baz::Baz;
mod biz;
use biz::Biz;

#[derive(Default, Debug)]
struct TestMetrics {
    hit_count: HitCount,
    error_count: ErrorCount,
}

fn print_openmetrics(prefix: &str, tree: &impl MetricTree) {
    let mut registry = Registry::new();
    registry.register(
        metered::entry::metric(prefix.to_owned())
            .source(tree)
            .help(format!("{prefix} metrics")),
    );
    println!(
        "{}",
        registry
            .encode_to_string()
            .expect("encode OpenMetrics text")
    );
}

fn test(should_fail: bool, metrics: &TestMetrics) -> Result<(), ()> {
    let hit_count = &metrics.hit_count;
    let error_count = &metrics.error_count;
    measure!(hit_count, {
        measure!(error_count, {
            println!("test !");
            if should_fail {
                Err(())
            } else {
                Ok(())
            }
        })
    })
}

fn test_incr(metrics: &TestMetrics) -> Result<(), ()> {
    metrics.hit_count.incr_by(3);
    Ok(())
}

fn sync_procmacro_demo(baz: &Baz) {
    for i in 1..=10 {
        baz.foo();
        let _ = baz.bar(i % 3 == 0);
    }
}

async fn async_procmacro_demo(baz: Baz) {
    for i in 1..=5 {
        let _ = baz.baz(i % 3 == 0).await;
        let _ = baz.bazle(i % 3 == 0).await;
    }

    print_openmetrics("baz", baz.metric_tree());
}

fn simple_api_demo() {
    let metrics = TestMetrics::default();

    let _ = test(false, &metrics);
    let _ = test(true, &metrics);
    let _ = test_incr(&metrics);

    let mut registry = Registry::with_prefix("test");
    registry.register(
        metered::entry::metric("hit")
            .source(&metrics.hit_count)
            .help("Hit count"),
    );
    registry.register(
        metered::entry::metric("error")
            .source(&metrics.error_count)
            .help("Error count"),
    );
    println!(
        "{}",
        registry
            .encode_to_string()
            .expect("encode OpenMetrics text")
    );
}

use std::sync::Arc;
use std::thread;

fn test_biz() {
    println!("Running Biz hit-count demo...(will take a few seconds)");
    let biz = Arc::new(Biz::default());
    do_test_biz(&biz);
}

fn do_test_biz(biz: &Arc<Biz>) {
    let mut threads = Vec::new();
    for _ in 0..5 {
        let biz = Arc::clone(&biz);
        let t = thread::spawn(move || {
            for _ in 0..200 {
                biz.biz();
            }
        });
        threads.push(t);
    }
    for t in threads {
        t.join().unwrap();
    }
    println!("Running Biz hit-count demo... done! OpenMetrics output:");
    print_openmetrics("biz", &biz.metrics);
}

fn main() {
    simple_api_demo();

    test_biz();

    let baz = Baz::default();

    sync_procmacro_demo(&baz);
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async_procmacro_demo(baz));
}
