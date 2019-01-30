use metered::*;

#[derive(Default, Debug)]
pub struct Baz {
    metrics: BazMetricRegistry,
}

#[metered(registry = BazMetricRegistry)]
impl Baz {
    #[measure(ResponseTime)]
    pub fn foo(&self) {
        println!("foo !");
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
        std::thread::sleep(delay);
    }

    #[measure(type = HitCount<atomic::Atomic<u128>>, debug = println!)]
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

    #[measure([ErrorCount, ResponseTime])]
    pub async fn baz(&self, should_fail: bool)  -> Result<(), &'static str> {
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);

        let when = std::time::Instant::now() + delay;
        tokio::await!(tokio::timer::Delay::new(when)).map_err(|_| "Tokio timer error")?;
        if !should_fail {
            println!("baz !");
            Ok(())
        } else {
            Err("I failed!")
        }
    }

}
