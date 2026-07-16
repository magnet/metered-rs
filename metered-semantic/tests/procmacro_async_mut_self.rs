//! Issue #13 for `async` measured methods: a measured `async fn` must be
//! allowed to take `&mut self` and to `.await` other `&mut self` methods from
//! inside the measured body. Async support is mandatory, so this exercises the
//! `&mut self` + `.await` combination end to end.

use metered_semantic::{metered, HitCount, InFlight};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

#[derive(Default, Debug)]
pub struct AsyncBoz {
    inc: i32,
    metrics: AsyncBozMetrics,
}

#[metered(registry = AsyncBozMetrics)]
impl AsyncBoz {
    #[measure([HitCount, InFlight])]
    pub async fn increment_once(&mut self) {
        self.inc += 1;
    }

    // A measured `async fn(&mut self)` that `.await`s another `&mut self`
    // method from inside the measured body -- the async form of issue #13.
    #[measure(HitCount)]
    pub async fn increment_twice(&mut self) {
        self.increment_once().await;
        self.increment_once().await;
    }
}

#[test]
fn procmacro_allows_async_mut_self() {
    let mut boz = AsyncBoz::default();
    block_on(boz.increment_once()); // inc -> 1, once-hit -> 1
    block_on(boz.increment_twice()); // twice-hit -> 1, two inner once-hits -> once-hit 3

    assert_eq!(
        boz.inc, 3,
        "every async &mut self body should have executed"
    );
    assert_eq!(boz.metrics.increment_once.hit_count.get(), 3);
    assert_eq!(boz.metrics.increment_twice.hit_count.get(), 1);
    assert_eq!(
        boz.metrics.increment_once.in_flight.get(),
        0,
        "in-flight gauge must be balanced after async completion"
    );
}

/// Minimal dependency-free executor: our futures have no real pending points,
/// so a single poll resolves them. Sufficient for testing the macro expansion.
fn block_on<F: Future>(future: F) -> F::Output {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => continue,
        }
    }
}

fn noop_waker() -> Waker {
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker {
        raw_waker()
    }
    fn raw_waker() -> RawWaker {
        RawWaker::new(
            std::ptr::null(),
            &RawWakerVTable::new(clone, no_op, no_op, no_op),
        )
    }
    // SAFETY: the vtable's functions are all no-ops / pure, satisfying the
    // RawWaker contract.
    unsafe { Waker::from_raw(raw_waker()) }
}

// Silence the unused `Pin` import warning on toolchains that infer it.
const _: fn() = || {
    let _ = Pin::new(&mut ());
};
