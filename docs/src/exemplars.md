# Exemplars

With the `legacy` feature, `Elapsed` can attach OpenMetrics exemplars to
histogram buckets. Metered does not
depend on tracing or OpenTelemetry; services provide an `ExemplarSource`.

```rust
use metered::bucket_histogram::{Exemplar, ExemplarSource};
use metered::{Buckets, Elapsed, ElapsedConfig};

#[derive(Clone)]
struct TraceSource {
    trace_id: &'static str,
}

impl ExemplarSource for TraceSource {
    fn exemplar(&self) -> Option<Exemplar> {
        Some(Exemplar {
            labels: vec![("trace_id".to_owned(), self.trace_id.to_owned())],
            value: 0.0,
            timestamp_seconds: None,
        })
    }
}

let elapsed = Elapsed::with_config(ElapsedConfig {
    buckets: Buckets::fast_seconds(),
    exemplar_source: TraceSource { trace_id: "abc123" },
});
```

`Elapsed` overwrites `Exemplar::value` with the actual observed duration, so the
source only supplies trace labels and an optional timestamp. One exemplar is kept
per bucket (the most recent), per the OpenMetrics rule, and it rides along on the
`_bucket` line in the exposition.

## Why exemplars instead of a label

An exemplar attaches a trace id to a single observation **without** creating a
new time series. Putting a trace id in a label would explode cardinality (see
[Labels and Families](./labels-families.md)); an exemplar is the sanctioned way
to jump from "this bucket got slow" to "here is an example trace".

## Ambient context

Wiring a `trace_id` through every call site is tedious. With the
`exemplar-context` feature, a tracing layer can set an ambient exemplar for the
current scope, and `Elapsed<ThreadLocalExemplars>` picks it up automatically:

```rust,ignore
use metered::bucket_histogram::{with_exemplar, Exemplar, ThreadLocalExemplars};
use metered_semantic::Elapsed;

let elapsed: Elapsed<ThreadLocalExemplars> = Elapsed::default();

let span_exemplar = Exemplar {
    labels: vec![("trace_id".to_owned(), "abc123".to_owned())],
    value: 0.0,
    timestamp_seconds: None,
};
with_exemplar(span_exemplar, || {
    // Any Elapsed<ThreadLocalExemplars> observed in here attaches the exemplar.
});
```

This keeps Metered free of any tracing/OpenTelemetry dependency: the feature is
just a thread-local seam a tracing integration can drive.
