#![allow(dead_code)]

use metered_semantic::{metered, Elapsed, ErrorCount, HitCount, InFlight};
use thiserror::Error;

#[metered_semantic::error_count(name = LibErrorCount, visibility = pub)]
#[derive(Debug, Error)]
pub enum LibError {
    #[error("I failed!")]
    Failure,
    #[error("Bad input")]
    BadInput,
}

#[metered_semantic::error_count(name = BazErrorCount, visibility = pub)]
#[derive(Debug, Error)]
pub enum BazError {
    #[error("lib error: {0}")]
    Lib(#[from] #[nested] LibError),
    #[error("io error")]
    Io,
}

#[derive(Default, Debug)]
pub struct Baz {
    metric_reg: BazMetricRegistry,
}

impl Baz {
    pub(crate) fn metric_tree(&self) -> &BazMetricRegistry {
        &self.metric_reg
    }
}

#[metered(
    registry = BazMetricRegistry,
    registry_expr = self.metric_reg,
    visibility = pub(crate)
)]
#[measure(InFlight)]
impl Baz {
    #[measure]
    pub fn bir(&self) {
        println!("bir");
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
        std::thread::sleep(delay);
    }

    pub fn bor(&self) {
        println!("bor");
    }

    #[measure(Elapsed)]
    pub fn foo(&self) {
        println!("foo !");
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
        std::thread::sleep(delay);
    }

    #[measure(HitCount)]
    #[measure(ErrorCount)]
    #[measure(Elapsed)]
    pub fn bar(&self, should_fail: bool) -> Result<(), &'static str> {
        if !should_fail {
            println!("bar !");
            Ok(())
        } else {
            Err("I failed!")
        }
    }

    #[measure([ErrorCount, Elapsed])]
    pub async fn baz(&self, should_fail: bool) -> Result<(), &'static str> {
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
        tokio::time::sleep(delay).await;
        if !should_fail {
            println!("baz !");
            Ok(())
        } else {
            Err("I failed!")
        }
    }

    #[measure([Elapsed])]
    pub fn bazium(
        &self,
        should_fail: bool,
    ) -> impl std::future::Future<Output = Result<(), &'static str>> {
        async move {
            let delay = std::time::Duration::from_millis(rand::random::<u64>() % 2000);
            tokio::time::sleep(delay).await;
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

    pub fn bur() {
        println!("bur");
    }
}
