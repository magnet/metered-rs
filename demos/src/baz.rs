#![allow(dead_code)]

use metered::{metered, ErrorCount, HitCount, InFlight, ResponseTime};
use thiserror::Error;

#[metered::error_count(name = LibErrorCount, visibility = pub)]
#[derive(Debug, Error)]
pub enum LibError {
    #[error("I failed!")]
    Failure,
    #[error("Bad input")]
    BadInput,
}

#[metered::error_count(name = BazErrorCount, visibility = pub, skip_cleared = true)]
#[derive(Debug, Error)]
pub enum BazError {
    #[error("lib error: {0}")]
    Lib(#[from] #[nested] LibError),
    #[error("io error")]
    Io,
}

#[derive(Default, Debug, serde::Serialize)]
pub struct Baz {
    metric_reg: BazMetricRegistry,
}

#[metered(registry = BazMetricRegistry, /* default = self.metrics */ registry_expr = self.metric_reg, visibility = pub(self))]
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

    #[measure(type = HitCount<metered::atomic::AtomicInt<u64>>)]
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
    pub async fn baz(&self, should_fail: bool) -> Result<(), &'static str> {
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
        tokio::time::delay_for(delay).await;
        if !should_fail {
            println!("baz !");
            Ok(())
        } else {
            Err("I failed!")
        }
    }

    #[measure([ResponseTime])]
    pub fn bazium(
        &self,
        should_fail: bool,
    ) -> impl std::future::Future<Output = Result<(), &'static str>> {
        async move {
            let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
            tokio::time::delay_for(delay).await;
            if !should_fail {
                println!("baz !");
                Ok(())
            } else {
                Err("I failed!")
            }
        }
    }

    #[measure([HitCount, BazErrorCount])]
    pub async fn bazle(&self, should_fail: bool) -> Result<(), BazError> {
        if !should_fail {
            println!("bazle !");
            Ok(())
        } else {
            Err(LibError::Failure.into())
        }
    }

    #[measure]
    pub unsafe fn bad(&self, v: &[u8]) {
        let _ = std::str::from_utf8_unchecked(v);
    }

    // This is not measured either
    pub fn bur() {
        println!("bur");
    }
}
