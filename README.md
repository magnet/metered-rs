# metered-rs
[![Build Status](https://travis-ci.org/magnet/metered-rs.svg?branch=master)](https://travis-ci.org/magnet/metered-rs)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/magnet/metered-rs)
[![Cargo](https://img.shields.io/crates/v/metered.svg)](
https://crates.io/crates/metered)
[![Documentation](https://docs.rs/metered/badge.svg)](
https://docs.rs/metered)
[![Rust 1.31+](https://img.shields.io/badge/rust-1.31+-lightgray.svg)](
https://www.rust-lang.org)

## Fast, ergonomic metrics for Rust!

Metered helps you measure the performance of your programs in production. Inspired by Coda Hale's Java metrics library, Metered makes live measurements easy by providing declarative and procedural macros to measure your program without altering your logic.

Metered is built with the following principles in mind:
 * **high ergonomics but no magic**: measuring code should just be a matter of annotating code. Metered lets you build your own metric registries from bare metrics, or will generate one using procedural macros. It does not use shared globals or statics.

 * **constant, very low overhead**: good ergonomics should not come with an overhead; the only overhead is the one imposed by actual metric back-ends themselves (e.g, counters, gauges, histograms), and those provided in Metered do not allocate after initialization.  Metered will generate metric registries as regular Rust `struct`s, so there is no lookup involved with finding a metric. Metered provides both unsynchronized and thread-safe metric back-ends so that single-threaded or share-nothing architectures don't pay for synchronization. Where possible, thread-safe metric back-ends provided by Metered use lock-free data-structures.

 * **extensible**: metrics are just regular types that implement the [`Metric`](https://docs.rs/metered/0.7.0/metered/metric/trait.Metric.html) trait with a specific behavior. Metered's macros let you refer to any Rust type, resulting in user-extensible attributes!

 Many metrics are only meaningful if we get precise statistics. When it comes to low-latency, high-range histograms, there's nothing better than [Gil Tene's High Dynamic Range Histograms](http://hdrhistogram.org/) and Metered uses [the official Rust port](https://github.com/HdrHistogram/HdrHistogram_rust) by default for its histograms.


## Changelog

* 0.7.0:
  * Expose inner metric backend `Throughput` type (fixes issue #30)
  * Implement `Deref` for all top-level metrics
  * Expose inner metric backend `Throughput` type
  * Add `skip_cleared` option to `error_count` attribute  (contributed by [@w4](https://github.com/w4))
     * Introduce a new `Clearable` trait that exposes behavior for metrics that implement `Clear` (in an effort of backwards compatibility).  Currently only implemented on counters.
     * Default behavior can be controlled by the a build-time feature, `error-count-skip-cleared-by-default`
* 0.6.0:
  * Extend `error_count` macro to allow `nested` enum error variants to be reported, providing zero-cost error tracking for nested errors (contributed by [@w4](https://github.com/w4))
* 0.5.0:
  * Make inner metrics public (contributed by [@nemosupremo](https://github.com/nemosupremo))
  * Provide `error_count` macro to generate a tailored `ErrorCount` metric counting variants for an error enum (contributed by [@w4](https://github.com/w4))
  * Use `Drop` to automatically trigger metrics that don't rely on the result value (affects `InFlight`, `ResponseTime`, `Throughput`)
* 0.4.0:
  * Add allow(missing_docs) to generated structs (This allows to use metered structs in Rust code with lint level warn(missing_docs) or even deny(missing_docs)) (contributed by [@reyk](https://github.com/reyk))
  * Implement `Clear` for generated registries (contributed by [@eliaslevy](https://github.com/eliaslevy))
  * Implement `Histogram` and `Clear` for `RefCell<HdrHistogram>` (contributed by [@eliaslevy](https://github.com/eliaslevy))
  * Introduce an `Instant` with microsecond precision (contributed by [@eliaslevy](https://github.com/eliaslevy))
     * API breaking change: `Instant.elapsed_millis` is renamed to `elapsed_time`, and a new associated constant, `ONE_SEC` is introduced to specify one second in the instant units.
  * Make `AtomicTxPerSec` and `TxPerSec` visible by reexporting  (contributed by [@eliaslevy](https://github.com/eliaslevy))
  * Add `StdInstant` as the default type parameter for `T: Instant` in `TxPerSec`  (contributed by [@eliaslevy](https://github.com/eliaslevy))
  * Modify HdrHistogram to work with serde_prometheus (contributed by [@w4](https://github.com/w4))
     * To be used with [serde_prometheus](https://github.com/w4/serde_prometheus) and any HTTP server.
  * Bumped dependencies:
     * `indexmap`: 1.1 -> 1.3 
     * `hdrhistogram`: 6.3 -> 7.1 
     * `parking_lot`: 0.9 -> 0.10  
* 0.3.0:
  * Fix to preserve span in `async` measured methods.
  * Update nightly sample for new syntax and Tokio 0.2-alpha (using std futures, will need Rust >= 1.39, nightly or not)
  * Updated dependencies to use `syn`, `proc-macro2` and `quote` 1.0
* 0.2.2:
  * Async support in `#measured` methods don't rely on async closures anymore, so client code will not require the `async_closure` feature gate.
  * Updated dependency versions
* 0.2.1:
  * Under certain circumstances, Serde would serialize "nulls" for `PhantomData` markers in `ResponseTime` and `Throughput` metrics. They are now explicitely excluded.
* 0.2.0:
  * Support for `.await` notation users (no more `await!()`)
* 0.1.3:
  * Fix for early returns in `#[measure]`'ed methods
  * Removed usage of crate `AtomicRefCell` which sometimes panicked .
  * Support for custom registry visibility.
  * Support for `async` + `await!()` macro users.


## Using Metered

Metered comes with a variety of useful metrics ready out-of-the-box:
* `HitCount`: a counter tracking how much a piece of code was hit.
* `ErrorCount`: a counter tracking how many errors were returned -- (works on any expression returning a std `Result`)
* `InFlight`: a gauge tracking how many requests are active 
* `ResponseTime`: statistics backed by an HdrHistogram of the duration of an expression
* `Throughput`: statistics backed by an HdrHistogram of how many times an expression is called per second.

These metrics are usually applied to methods, using provided procedural macros that generate the boilerplate.

To achieve higher performance, these stock metrics can be customized to use non-thread safe (`!Sync`/`!Send`) datastructures, but they default to thread-safe datastructures implemented using lock-free strategies where possible. This is an ergonomical choice to provide defaults that work in all situations.

Metered is designed as a zero-overhead abstraction -- in the sense that the higher-level ergonomics should not cost over manually adding metrics. Notably, stock metrics will *not* allocate memory after they're initialized the first time.  However, they are triggered at every method call and it can be interesting to use lighter metrics (e.g `HitCount`) in hot code paths and favour heavier metrics (`Throughput`, `ResponseTime`) in higher-level entry points.

If a metric you need is missing, or if you want to customize a metric (for instance, to track how many times a specific error occurs, or react depending on your return type), it is possible to implement your own metrics simply by implementing the trait `metered::metric::Metric`.

Metered does not use statics or shared global state. Instead, it lets you either build your own metric registry using the metrics you need, or can generate a metric registry for you using method attributes. Metered will generate one registry per `impl` block annotated with the `metered` attribute, under the name provided as the `registry` parameter. By default, Metered will expect the registry to be accessed as `self.metrics` but the expression can be overridden with the `registry_expr` attribute parameter. See the demos for more examples.

Metered will generate metric registries that derive `Debug` and `serde::Serialize` to extract your metrics easily. Metered generates one sub-registry per method annotated with the `measure` attribute, hence organizing metrics hierarchically. This ensures access time to metrics in generated registries is always constant (and, when possible, cache-friendly), without any overhead other than the metric itself.

Metered will happily measure any method, whether it is `async` or not, and the metrics will work as expected (e.g, `ResponseTime` will return the completion time across `await`'ed invocations).

Right now, Metered does not provide bridges to external metric storage or monitoring systems. Such support is planned in separate modules (contributions welcome!).

## Required Rust version

Metered works on `Rust` stable, starting 1.31.0.

It does not use any nightly features. There may be a `nightly` feature flag at some point to use upcoming Rust features (such as `const fn`s), and similar features from crates Metered depends on, but this is low priority (contributions welcome).

## Example using procedural macros (recommended)

```rust
use metered::{metered, Throughput, HitCount};

#[derive(Default, Debug, serde::Serialize)]
pub struct Biz {
    metrics: BizMetrics,
}

#[metered(registry = BizMetrics)]
impl Biz {
    #[measure([HitCount, Throughput])]
    pub fn biz(&self) {        
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 200);
        std::thread::sleep(delay);
    }   
}
```

In the snippet above, we will measure the `HitCount` and `Throughput` of the `biz` method.

This works by first annotating the `impl` block with the `metered` annotation and specifying the name Metered should give to the metric registry (here `BizMetrics`). Later, Metered will assume the expression to access that repository is `self.metrics`, hence we need a `metrics` field with the `BizMetrics` type in `Biz`. It would be possible to use another field name by specificying another registry expression, such as `#[metered(registry = BizMetrics, registry_expr = self.my_custom_metrics)]`.

Then, we must annotate which methods we wish to measure using the `measure` attribute, specifying the metrics we wish to apply: the metrics here are simply types of structures implementing the `Metric` trait, and you can define your own. Since there is no magic, we must ensure `self.metrics` can be accessed, and this will only work on methods with a `&self` or `&mut self` receiver.

Let's look at `biz`'s code a second: it's a blocking method that returns after between 0 and 200ms, using `rand::random`. Since `random` has a random distribution, we can expect the mean sleep time to be around 100ms. That would mean around 10 calls per second per thread.

In the following test, we spawn 5 threads that each will call `biz()` 200 times. We thus can expect a hit count of 1000, that it will take around 20 seconds (which means 20 samples, since we collect one sample per second), and around 50 calls per second (10 per thread, with 5 threads).

```rust
use std::thread;
use std::sync::Arc;

fn test_biz() {
    let biz = Arc::new(Biz::default());
    let mut threads = Vec::new();
    for _ in 0..5 {
        let biz = Arc::clone(&biz);
        let t = thread::spawn(move || {
            for _ in 0..200 {
                biz.biz();
            }
        });
        threads.push(t);
    }
    for t in threads {
        t.join().unwrap();
    }
    // Print the results!
    let serialized = serde_yaml::to_string(&*biz).unwrap();
    println!("{}", serialized);
}
```

We can then use serde to serialize our type as YAML:
```yaml
metrics:
  biz:
    hit_count: 1000
    throughput:
      - samples: 20
        min: 35
        max: 58
        mean: 49.75
        stdev: 5.146600819958742
        90%ile: 55
        95%ile: 55
        99%ile: 58
        99.9%ile: 58
        99.99%ile: 58
      - ~
```

We see we indead have a mean of 49.75 calls per second, which corresponds to our expectations.

The Hdr Histogram backing these statistics is able to give much more than fixed percentiles, but this is a practical view when using text. For a better performance analysis, please watch Gil Tene's talks ;-).

## Macro Reference

### The `metered` attribute

`#[metered(registry = YourRegistryName, registry_expr = self.wrapper.my_registry)]` 

`registry` is mandatory and must be a valid Rust ident.

`registry_expr` defaults to `self.metrics`, alternate values must be a valid Rust expression. This setting lets you configure the expression which resolves to the registry. Please note that this triggers an immutable borrow of that expression.

`visibility` defaults to `pub(crate)`, and must be a valid struct Rust visibility (e.g, `pub`, `<nothing>`, `pub(self)`, etc). This setting lets you alter the visibility of the generated registry `struct`s. The registry fields are always public and named after snake cased methods or metrics.

### The `measure` attribute

Single metric:

`#[measure(path::to::MyMetric<u64>)]`

or: 

`#[measure(type = path::to::MyMetric<u64>)]`

Multiple metrics:

`#[measure([path::to::MyMetric<u64>, path::AnotherMetric])]`

or

`#[measure(type = [path::to::MyMetric<u64>, path::AnotherMetric])]`

The `type` keyword is allowed because other keywords are planned for future extra attributes (e.g, instantation options).

When `measure` attribute is applied to an `impl` block, it applies for every method that has a `measure` attribute. If a method does not need extra measure infos, it is possible to annotate it with simply `#[measure]` and the `impl` block's `measure` configuration will be applied.

The `measure` keyword can be added several times on an `impl` block or method, which will add to the list of metrics applied. Adding the same metric several time will lead in a name clash.

### Design

Metered's custom attribute parsing supports using reserved keywords and arbitrary Rust syntax. The code has been extracted to the [Synattra](https://github.com/magnet/synattra) project, which provides useful methods on top of the Syn parser for Attribute parsing.

Metered's metrics can wrap any piece of code, regardless of whether they're `async` blocks or not, using hygienic macros to emulate an approach similar to aspect-oriented programming. That code has been extracted to the [Aspect-rs](https://github.com/magnet/aspect-rs) project!


## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
