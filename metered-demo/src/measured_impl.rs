use metered_core::*;
use metered_macro::measured;

#[derive(Default, Debug)]
pub struct Baz {
    registry: MetricRegistry,
}

#[measured]
impl Baz {
    #[measure(ReponseTime)]
    pub fn foo(&self) {
        println!("foo !");
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
        std::thread::sleep(delay);
    }

    #[measure(type = HitCount<atomic::Atomic<u128>>, debug = println!)]
    #[measure(ErrorCount)]
    #[measure(ReponseTime)]
    pub fn bar(&self, should_fail: bool) -> Result<(), &'static str> {
        if !should_fail {
            println!("bar !");
            Ok(())
        } else {
            Err("I failed!")
        }
    }

    #[measure(ReponseTime)]
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
