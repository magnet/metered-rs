# Metered

Metered is a metric-state, composition, and schema/value collection library for
Rust services. Core `metered` gives services readable metric state, typed metric
trees, schema/value collection, and registry views that borrow through the real
service graph at scrape time. Exposition formats live in sink crates such as
`metered-om`.

Operation instrumentation is layered: use `metered-tracing` for RPC/HTTP/server
measurements derived from `tracing` spans, and the `metered-semantic` crate for
method-level semantic wrappers, the `#[metered]` / `#[error_count]` / `measure!`
macros, and explicit `recording`.

This book is meant to do two things:

1. Teach you to **use** Metered well.
2. Teach you to **build great metrics** -- and explain *why* Metered is shaped
   the way it is, so the choices feel inevitable rather than arbitrary.

If you have never instrumented a service before, that is fine: the
[Metrics & OpenMetrics Primer](./metrics-primer.md) starts from zero.

## A first taste

```rust
use metered::entry::{counter, gauge};
use metered::{MetricTreeView, Unit};
use std::sync::atomic::AtomicU64;

struct Api {
    requests: AtomicU64,
    in_flight: AtomicU64,
}

fn metrics() -> MetricTreeView<'static, Api> {
    let mut view = MetricTreeView::with_prefix("api");
    view.register(counter("requests").select(|api: &Api| &api.requests).help("Requests"));
    view.register(gauge("in_flight").select(|api: &Api| &api.in_flight).help("In-flight requests").unit(Unit::Items));
    view
}
```

That view exposes the state the service already owns: request totals and current
in-flight work. The default model starts with owned state and explicit
composition; operation measurement is added through tracing spans or the
`metered-semantic` crate (semantic wrappers, method macros, explicit recording).

To see the whole picture, run the demo:

```bash
cargo run -p order-service-demo
```

It prints the OpenMetrics document for a small e-commerce service -- spans turned
into metrics, components owning their layout, and a dynamic payment-rail fleet
fanned out by label -- each pattern documented module by module
(see [Demo App](./demo.md)).

## How to read this book

- **Concepts** explains the model and the reasoning behind it. Read
  [Why Metered Is Designed This Way](./design.md) early; it is the key to the
  rest.
- **Building Great Metrics** is the practical craft: which metric type to reach
  for, how to keep labels safe, and how to keep wire names stable as code
  changes.
- **Instrumenting & Exposing** covers the mechanics: tracing-derived operation
  metrics, explicit recording, the `metered-semantic` method macros, registries,
  exemplars, and turning a schema into dashboards.
- **Reference** covers the demo, migrating from older versions, and the feature
  flags.

## What Metered deliberately avoids

- **Global registries and statics.** Metrics are fields on your types.
- **`serde` on the default path.** Exposition is native OpenMetrics text.
- **Reset/clear semantics.** Counters and histograms are cumulative; rates and
  quantiles are computed at query time, where they aggregate correctly across
  replicas.
- **Foreign types leaking through the public API.** A `metered` upgrade does not
  drag `serde` or `hdrhistogram` along.

The next chapter explains why each of those is a feature, not a limitation.
