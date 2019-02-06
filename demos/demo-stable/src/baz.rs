#![allow(dead_code)]

use metered::{metered, ErrorCount, HitCount, InFlight, ResponseTime};

#[derive(Default, Debug, serde::Serialize)]
pub struct Baz {
    metric_reg: BazMetricRegistry,
}

#[metered(registry = BazMetricRegistry, /* default = self.metrics */ registry_expr = self.metric_reg)]
#[measure(InFlight)] // Applies to all methods that have the `measure` attribute
impl Baz {
    // This is measured with an InFlight gauge, because it's the default on the block.
    #[measure]
    pub fn bir(&self) {
        println!("bir");
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
        std::thread::sleep(delay);
    }

    // This is not measured
    pub fn bor(&self) {
        println!("bor");
    }

    #[measure(ResponseTime)]
    pub fn foo(&self) {
        println!("foo !");
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
        std::thread::sleep(delay);
    }

    #[measure(type = HitCount<metered::atomic::AtomicInt<u128>>)]
    #[measure(ErrorCount)]
    #[measure(ResponseTime)]
    pub fn bar(&self, should_fail: bool) -> Result<(), &'static str> {
        if !should_fail {
            println!("bar !");
            Ok(())
        } else {
            Err("I failed!")
        }
    }

    // This is not measured either
    pub fn bur() {
        println!("bur");
    }
}
