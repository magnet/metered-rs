//! Simple hot-path benchmark for histogram backends.
//!
//! This intentionally avoids a benchmark framework dependency. It is not meant
//! to replace Criterion-quality numbers; it is a quick, repeatable smoke test
//! for the relative write-path costs of the three in-memory histogram backends.

use metered::{
    BucketHistogram, Buckets, DynamicExponentialHistogram, FixedExponentialHistogram, Histogram,
};
use std::hint::black_box;
use std::time::{Duration, Instant};

const ITERS: usize = 1_000_000;

fn values() -> Vec<f64> {
    (0..ITERS)
        .map(|i| {
            // Deterministic latency-ish range: ~1us .. ~30s with repeated buckets.
            let octave = (i % 320) as i32;
            1e-6 * 1.05f64.powi(octave)
        })
        .collect()
}

fn run(name: &str, values: &[f64], mut observe: impl FnMut(f64)) {
    let start = Instant::now();
    for &value in values {
        observe(black_box(value));
    }
    let elapsed = start.elapsed();
    print_result(name, elapsed);
}

fn print_result(name: &str, elapsed: Duration) {
    let nanos = elapsed.as_nanos() as f64 / ITERS as f64;
    let ops = ITERS as f64 / elapsed.as_secs_f64();
    println!("{name:<28} {:>10.1} ns/op {:>12.0} obs/s", nanos, ops);
}

fn main() {
    println!("histogram observe hot path ({ITERS} observations)");
    println!("{}", "-".repeat(58));
    let values = values();

    let bucket = BucketHistogram::new(Buckets::relative(0.000_001, 30.0, 0.10));
    run("BucketHistogram", &values, |value| {
        bucket.observe(value);
    });

    let fixed = FixedExponentialHistogram::new(0.000_001, 30.0, 5);
    run("FixedExponentialHistogram", &values, |value| {
        fixed.observe(value);
    });

    let dynamic = DynamicExponentialHistogram::with_params(5, 512);
    run("DynamicExponentialHistogram", &values, |value| {
        dynamic.observe(value);
    });
    dynamic.rescale_if_needed();

    println!("{}", "-".repeat(58));
    println!(
        "counts: bucket={} fixed={} dynamic={}",
        Histogram::count(&bucket),
        Histogram::count(&fixed),
        Histogram::count(&dynamic)
    );
}
