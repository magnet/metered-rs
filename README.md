# metered-rs
## Fast, ergonomic metrics for Rust!

Metered helps you measure the performance of your programs in production. Inspired by Coda Hale's Java metrics library, Metered makes live measurements easy by providing measurement declarative and procedural macros, and a variety of useful metrics ready out-of-the-box:
* `HitCount`: a counter tracking how much a method was hit.
* `ErrorCount`: a counter tracking how many errors were returned -- (works on any method returning a std `Result`)
* `InFlight`: a gauge tracking how many requests are active for a method
* `ResponseTime`: statistics backed by an HdrHistogram of the duration of a method call
* `Throughput`: statistics backed by an HdrHistogram of how many times a method is called per second.

For better performance, these stock metrics can be customized to use a non-thread safe (`!Sync`/`!Send`) datastructure. For ergonomy reasons, stock metrics default to thread-safe datastructures, implemented using lock-free strategies where possible.

Metered is designed as a zero-overhead abstraction -- in the sense that the higher-level ergonomics should not cost over manually adding metrics. Stock metrics will *not* allocate memory after they're initialized the first time.  However, they are triggered at every method call and it can be interesting to use lighter metrics (e.g `HitCount`) in very hot code paths and favour heavier metrics (`Throughput`, `ResponseTime`) in entry points.

If a metric you need is missing, or if you want to customize a metric (for instance, to track how many times a specific error happens, or react depending on your return type), it is possible to implement your own metrics simply by implementing the trait `metered::metric::Metric`.

Metered does not use statics or shared global state. Instead, it lets you either build your own metric registry using the metrics you need, or can generate a metric registry for you using method attributes. Metered will generate one registry per `impl` block annotated with the `metered` attribute, under the name provided as the `registry` parameter. By default, Metered will expect the registry to be accessed as `self.metrics` but the expression can be overridden with the `registry_expr` attribute parameter. See the demo for more examples.

Metered will generate metric registries that derive `Debug` and `serde::Serialize` to extract your metrics easily. Adapters for metric storage and monitoring systems are planned (contributions welcome!). Metered generates one sub-registry per method annotated with the `measure` attribute, hence organizing metrics hierarchically. This ensures access to metrics is always constant,without any overhead other than the metric itself.

Metered will happily measure any method, whether it is `async` or not, and the metrics will work as expected (e.g, `ResponseTime` will return the completion time across `await!` invocations).

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

Then, we must annotate which methods we wish to measure using the `measure` attribute, and specifying the metrics we wish to apply: the metrics here are simply types of structures implementing the `Metric` trait, and you can define your own. Since there is no magic, we must ensure `self.metrics` can be accessed, and this will only work on methods with a `&self` or `&mut self` receiver.

Let's look at `biz`'s code a second: it's a blocking method that returns after between 0 and 200ms, using `rand::random`. Since `random` has a random distribution, we can expect the mean sleep time to be around 100ms. That would mean around 10 calls per second per thread.

In the following test, we spawn 5 threads that each will call `biz()` 20 times. We thus can expect a hit count of 100, and around 50 calls per second (10 per thread, with 5 threads).

```rust
fn test_biz() {
    use std::thread;
    use std::sync::Arc;
    use std::ops::Deref;

    let biz = Arc::new(Biz::default());

    let mut threads = Vec::new();
    for _ in 0..5 {
        let biz = Arc::clone(&biz);
        let t = thread::spawn(move || {
            for _ in 0..20 {
                biz.biz();
            }
        });
        threads.push(t);
    }

    for t in threads {
        let _ = t.join().unwrap();
    }

    // Print the results!
    let serialized = serde_yaml::to_string(biz.deref()).unwrap();
    println!("{}", serialized);
}
```

We can then use serde to serialize our type as YAML:
```yaml
metrics:
  biz:
    hit_count: 100
    throughput:
      - samples: 2
        min: 46
        max: 52
        mean: 49.0
        stdev: 3.0
        90%ile: 52
        95%ile: 52
        99%ile: 52
        99.9ile: 52
        99.99ile: 52
      - ~
```

We see we indead have a mean of 49 calls per second, which corresponds to our expectations.

The Hdr Histogram backing these statistics is able to give much more than fixed percentiles, but this is a practical view when using text. For a better performance analysis, please watch Gil Tene's talks ;-).

## Macro Reference

### The `metered` attribute

`#[metered(registry = YourRegistryName, registry_expr = self.wrapper.my_registry)]` 
`registry` is mandatory and must be a valid Rust ident.
`registry_expr` defaults to `this.metrics`, alternate values must be a valid Rust expression.

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


### Design

Do you like Metered's custom attribute parsing, which lets you use reserved keywords and arbitrary Rust syntax? The code has been extracted to the Synattra crate, which provides useful methods on top of the Syn parser for Attribute parsing!

Do you want to build a project that wraps method calls the same way Metrics does, regardless of whether they're `async` blocks or not? That code has been extracted to the Aspect-rs project!


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