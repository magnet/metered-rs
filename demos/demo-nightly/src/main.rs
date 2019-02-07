#![feature(await_macro, async_await, futures_api)]

use metered::*;
mod baz;
use baz::Baz;
mod biz;
use biz::Biz;


#[derive(Default, Debug, serde::Serialize)]
struct TestMetrics {
    hit_count: HitCount,
    error_count: ErrorCount,
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

fn sync_procmacro_demo(baz: &Baz) {
    for i in 1..=10 {
        baz.foo();
        let _ = baz.bar(i % 3 == 0);
    }
}

async fn async_procmacro_demo(baz: Baz) {

    for i in 1..=5 {
        let _ = await!(baz.baz(i % 3 == 0));
    }

    // Print the results!
    let serialized = serde_yaml::to_string(&baz).unwrap();
    println!("{}", serialized);
}

fn simple_api_demo() {
    let metrics = TestMetrics::default();

    let _ = test(false, &metrics);
    let _ = test(true, &metrics);
    // Print the results!
    let serialized = serde_yaml::to_string(&metrics).unwrap();
    println!("{}", serialized);
}

use std::thread;
use std::sync::Arc;

fn test_biz() {
    println!("Running Biz throughput demo...(will take 20 seconds)");
    let biz = Arc::new(Biz::default());
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
    println!("Running Biz throughput demo... done! Here are the metrics for that run:");
    // Print the results!
    let serialized = serde_yaml::to_string(&*biz).unwrap();
    println!("{}", serialized);
}

fn main() {
    simple_api_demo();

    test_biz();

    let baz = Baz::default();

    sync_procmacro_demo(&baz);

    tokio::run_async(async_procmacro_demo(baz));
}
