#![feature(await_macro, async_await, futures_api)]

use metered::*;
mod metered_impl;
use metered_impl::Baz;

#[derive(Default, Debug)]
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



async fn async_demo() {
    let baz = Baz::default();
   
    for i in 1..=5 {
        // let _ = await!(baz.baz(i % 3 == 0));
        let _ = await!(baz.baz(i % 3 == 0));

    } 
    println!("baz: {:#?}", baz);

}



fn main() {
    let metrics = TestMetrics::default();

    let _ = test(false, &metrics);
    let _ = test(true, &metrics);

    println!("c {:#?}", metrics);
    let baz = Baz::default();

    for i in 1..=30 {
        baz.foo();
        let _ = baz.bar(i % 3 == 0);
    }

    println!("baz: {:#?}", baz);


    tokio::run_async(async_demo());
}
