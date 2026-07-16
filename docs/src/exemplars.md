# Exemplars

A `BucketHistogram` can attach OpenMetrics exemplars to its buckets. Metered
does not depend on tracing or OpenTelemetry: the service or an integration
layer mints the `Exemplar` and hands it to the observation.

```rust
use metered::bucket_histogram::Exemplar;
use metered::{BucketHistogram, Buckets};

let latency = BucketHistogram::new(Buckets::fast_seconds());

let observed = 0.012; // seconds
latency.observe_with_exemplar(
    observed,
    Exemplar {
        labels: vec![("trace_id".to_owned(), "abc123".to_owned())],
        value: observed,
        timestamp_seconds: None,
    },
);
```

`observe_with_exemplar` counts exactly like `observe` and publishes the
exemplar into the bucket the value lands in, with one lock-free swap. Each
bucket keeps one exemplar, the most recent, as the OpenMetrics rule requires.
The exposition prints it on the `_bucket` line. Set `Exemplar::value` to
the observed value yourself -- the histogram stores the exemplar as given.

`observe` and `observe_with_exemplar` both return the landing bucket's index. A
sampling layer can then decide cheaply, without a second lookup, whether an
observation fell into an outlier bucket worth keeping a trace for.

## Supplying exemplars: `ExemplarSource`

When exemplars come from ambient context rather than a call-site literal,
implement `ExemplarSource`. This is a cheap, usually stateless type. It mints
an exemplar, with trace labels and an optional timestamp, for the observation
just recorded.

```rust
use metered::bucket_histogram::{Exemplar, ExemplarSource};

#[derive(Clone)]
struct TraceSource {
    trace_id: &'static str,
}

impl ExemplarSource for TraceSource {
    fn exemplar(&self) -> Option<Exemplar> {
        Some(Exemplar {
            labels: vec![("trace_id".to_owned(), self.trace_id.to_owned())],
            value: 0.0, // the caller fills in the observed value
            timestamp_seconds: None,
        })
    }
}
```

The default source, `NoExemplars`, never produces one.

## Why exemplars instead of a label

An exemplar attaches a trace id to a single observation and creates **no** new
time series. A trace id in a label would explode cardinality (see
[Labels and Families](./labels-families.md)). An exemplar is the sanctioned way
to move from a slow bucket to an example trace.

## Ambient context

Wiring a `trace_id` through every call site is tedious. With the
`exemplar-context` feature, a tracing layer can set an ambient exemplar for the
current scope (`set_current_exemplar` / `with_exemplar`). Any observation
point can read it back through the `ThreadLocalExemplars` source:

```rust,ignore
use metered::bucket_histogram::{with_exemplar, Exemplar, ExemplarSource, ThreadLocalExemplars};
use metered::BucketHistogram;

let latency = BucketHistogram::default();

let span_exemplar = Exemplar {
    labels: vec![("trace_id".to_owned(), "abc123".to_owned())],
    value: 0.0,
    timestamp_seconds: None,
};
with_exemplar(span_exemplar, || {
    // Inside the scope, read the ambient exemplar and attach it.
    let observed = 0.012;
    if let Some(mut exemplar) = ThreadLocalExemplars.exemplar() {
        exemplar.value = observed;
        latency.observe_with_exemplar(observed, exemplar);
    } else {
        latency.observe(observed);
    }
});
```

This keeps Metered free of any tracing/OpenTelemetry dependency: the feature is
just a thread-local seam a tracing integration can drive.

## The span-integrated path

For span-derived metrics, you do not write any of this by hand. The
`exemplar` feature of `metered-tracing` drives the ambient context from the
active span's fields. It attaches trace and span-id exemplars to the duration
histograms it records. See [Tracing Integration](./tracing.md).
