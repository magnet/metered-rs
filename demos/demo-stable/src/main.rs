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

fn simple_api_demo() {
    let metrics = TestMetrics::default();

    let _ = test(false, &metrics);
    let _ = test(true, &metrics);
    // Print the results!
    let serialized = serde_yaml::to_string(&metrics).unwrap();
    println!("{}", serialized);
}

fn test_biz() {
    use std::ops::Deref;
    use std::sync::Arc;
    use std::thread;

    let biz = Arc::new(Biz::default());

    let mut threads = Vec::new();
    for _ in 0..5 {
        let biz = Arc::clone(&biz);
        let t = thread::spawn(move || {
            for _ in 0..20 {
                biz.biz();
            }
        });
        threads.push(t);
    }

    for t in threads {
        let _ = t.join().unwrap();
    }

    // Print the results!
    let serialized = serde_yaml::to_string(biz.deref()).unwrap();
    println!("{}", serialized);
}

fn main() {
    simple_api_demo();

    test_biz();

    let baz = Baz::default();

    sync_procmacro_demo(&baz);
}
