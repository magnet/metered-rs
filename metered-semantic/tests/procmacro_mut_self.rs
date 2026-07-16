//! Regression test for issue #13 via the `#[metered]` procedural macro: a
//! measured *synchronous* method must be allowed to take `&mut self` and to
//! call other `&mut self` methods. This mirrors the `Boz` example from the
//! (closed) PR #14.

use metered_semantic::{metered, HitCount, InFlight};

#[derive(Default, Debug)]
pub struct Boz {
    inc: i32,
    metrics: BozMetrics,
}

#[metered(registry = BozMetrics)]
impl Boz {
    #[measure(HitCount)]
    pub fn increment_once(&mut self) {
        self.inc += 1;
    }

    // Calls a `&mut self` method from inside a measured `&mut self` method.
    #[measure([HitCount, InFlight])]
    pub fn increment_twice(&mut self) {
        self.increment_once();
        self.increment_once();
    }
}

#[test]
fn procmacro_allows_mut_self() {
    let mut boz = Boz::default();
    boz.increment_once(); // inc -> 1, once-hit -> 1
    boz.increment_twice(); // twice-hit -> 1, two inner once-hits -> once-hit 3, inc -> 3

    assert_eq!(boz.inc, 3, "every &mut self body should have executed");
    assert_eq!(
        boz.metrics.increment_once.hit_count.get(),
        3,
        "increment_once measured three times"
    );
    assert_eq!(
        boz.metrics.increment_twice.hit_count.get(),
        1,
        "increment_twice measured once"
    );
    // The InFlight gauge must be balanced back to zero after the call returns.
    assert_eq!(
        boz.metrics.increment_twice.in_flight.get(),
        0,
        "in-flight gauge must be balanced (no leak)"
    );
}
