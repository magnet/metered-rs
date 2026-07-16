# Adding Metrics

This guide is written for agents and humans adding observability to Rust
services with Metered. Follow it before introducing a new metric.

## The Rule

Metrics belong to the object that owns the behavior or state being observed.
Avoid detached “metrics bags” that measure unrelated components. Prefer a small
metric tree or view next to the service state that already owns the behavior.

## Choose The Shape

Start with the state the service owns:

- A concrete `Counter` implementor, such as `AtomicU64`, for monotonic counts.
- A concrete `Gauge` implementor, such as `AtomicI64` or `AtomicU64`, for current
  values the service owns.
- Histograms for distributions. Duration metric names should include units,
  usually `_seconds`.
- `Info` for static build/version facts.
- `StateSet` for enum-like lifecycle state.
- `Family<L, M>` for bounded label dimensions.
- Existing state directly when the value already exists, such as `AtomicBool`,
  an atomic depth cache, or a queue length read while holding the queue lock.

Then choose operation instrumentation only if you are measuring an operation
boundary:

- For RPC/HTTP/server operations, prefer spans plus `metered-tracing`.
- For explicit non-tracing operation measurement, enable the `metered-semantic`
  `recording` feature and use `metered_semantic::recording::Operation`.
- For method-level wrappers and `#[metered]` / `#[error_count]`, add the
  `metered-semantic` crate.

Do not mirror mutable service state into a separate metric just to expose it. If
a queue already owns its length, expose a `QueueDepth` metric that reads the
queue length. If that lock is too expensive for scrapes, update a cached atomic
depth during queue mutations and expose that cache through a `Metric` newtype.

## Gauge Guidance

Gauges are for current state, not for event bookkeeping.

Good gauge examples:

- in-flight requests
- current queue depth
- enabled/disabled flag
- active workers
- cache entries

Bad gauge examples:

- total requests handled
- failed publishes
- retries
- dropped messages

Those are counters.

## Labels

Labels must be **bounded** and operationally useful: `operation`, `result`,
`route`, `upstream`, `mode`. Never label with raw user/account/request ids, URLs,
payloads, raw errors, or any open-ended input -- that explodes cardinality. If
the value set is open-ended, you need a different metric, a normalized category,
or no label. See [Labels and Families](./labels-families.md) for the full
treatment and for adding label dimensions with `Family`.

## Preferred Service Pattern

Expose a borrowed metric view over the service. The view stores no metrics and
does not require shared ownership.

```rust
use std::sync::atomic::AtomicU64;
use metered::entry::{counter, gauge};
use metered::{MetricTreeView, Unit};

struct Service {
    processed: AtomicU64,
    queue_depth: AtomicU64,
}

let mut view = MetricTreeView::with_prefix("service");
view.register(
    counter("processed")
        .select(|service: &Service| &service.processed)
        .help("Processed jobs")
        .unit(Unit::Items),
);
view.register(
    gauge("queue_depth")
        .select(|service: &Service| &service.queue_depth)
        .help("Jobs waiting in the queue")
        .unit(Unit::Items),
);
```

For a stable struct of borrowed metrics, derive `MetricTree`:

```rust
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use metered::MetricTree;

#[derive(MetricTree)]
struct ServiceMetrics<'a> {
    enabled: &'a AtomicBool,
    queue_depth: &'a AtomicUsize,
    processed: &'a AtomicU64,
}
```

Use this when you want a named metric tree type that can be registered or nested
inside another tree. To keep wire names stable while you refactor such a struct,
see [Shaping Names](./name-shaping.md).

## Custom Metrics

For one OpenMetrics family, implement `Metric`. This keeps the type and values in
one place. A queue depth encoded as a gauge is a gauge; the implementation just
decides where the current value comes from:

```rust
use metered::{Metric, MetricType, MetricValues};

struct QueueDepth<'a>(&'a std::sync::atomic::AtomicUsize);

impl Metric for QueueDepth<'_> {
    fn metric_type(&self) -> MetricType {
        MetricType::Gauge
    }

    fn collect_metric(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        values.gauge(name, labels, self.0.load(std::sync::atomic::Ordering::Relaxed));
    }
}
```

For a tree that emits multiple families, implement or derive `MetricTree`.

To keep emitted names stable as you rename fields/methods or reorganize structs,
use [Shaping Names](./name-shaping.md) (`#[metric(rename/flatten)]`).

## Checklist

Before you finish:

- The metric owner is the service/object that owns the behavior or state.
- Labels are bounded and useful on dashboards.
- Durations include units in the metric name.
- Gauges represent current state.
- Counters represent cumulative events.
- Existing state is exposed directly or through a deliberate cached value.
- The domain operation remains readable.
- Tests cover emitted names, labels, classification, and schema when relevant.
- Dashboards or docs are updated when names/labels change.
