//! Regression test for issue #13: the `measure!` macro must allow the measured
//! expression to take `&mut self` or call `&mut self` methods.
//!
//! Before the two-phase recorder model, `measure!` held a shared borrow of the metric
//! (and therefore of `self`) across the measured block, so any `&mut self`
//! access inside it failed to borrow-check.

use metered_semantic::{measure, HitCount};

#[derive(Default)]
struct Widget {
    value: i32,
    metrics: HitCount,
}

impl Widget {
    /// Mutates a *different* field of `self` inside the measured block.
    fn bump(&mut self) {
        measure!(&self.metrics, {
            self.value += 1;
        });
    }

    /// Calls a `&mut self` method from inside the measured block -- the exact
    /// case reported in issue #13.
    fn bump_twice(&mut self) {
        measure!(&self.metrics, {
            self.bump();
            self.bump();
        });
    }
}

#[test]
fn measure_allows_mut_self_in_body() {
    let mut w = Widget::default();
    w.bump(); // hit +1 (value: 1)
    w.bump_twice(); // outer hit +1, two inner bumps hit +2 (value: 3)

    assert_eq!(w.value, 3, "the &mut self body should have run three times");
    assert_eq!(
        w.metrics.get(),
        4,
        "every measured entry should have been counted once"
    );
}

#[test]
fn measure_returns_the_inner_value_unchanged() {
    let metrics: HitCount = HitCount::default();
    let out = measure!(&metrics, { 21 * 2 });
    assert_eq!(out, 42);
    assert_eq!(metrics.get(), 1);
}
