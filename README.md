# metered-rs

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/magnet/metered-rs)
[![Cargo](https://img.shields.io/crates/v/metered.svg)](https://crates.io/crates/metered)
[![Documentation](https://docs.rs/metered/badge.svg)](https://docs.rs/metered)

## Metric state and composition for Rust services

Metered is a metric-state, composition, and schema/value collection library for
Rust services. Exposition formats live in sink crates such as
`metered-om`.

Core `metered` gives services readable metric state (`Counter`, `Gauge`,
histograms, `Family`), typed metric trees, schema/value collection, and registry
views that borrow through the real service graph at scrape time.

Operation instrumentation is layered:

- use `metered-tracing` for service RPC/HTTP/server measurements derived from
  `tracing` spans;
- use `metered::recording` for explicit non-tracing operation measurement;
- use `legacy` for the old method-level wrappers and `#[metered]` /
  `#[error_count]` compatibility APIs.

Metered is built on a few ideas:

* **Metrics are state, not shadows.** A metric is the live value your code uses:
  an enabled flag can be a gauge-valued atomic that the service reads, and a
  queue-depth gauge should come from the queue or its cached depth. Metrics are
  readable, not write-only.

* **Native OpenMetrics, zero serialization.** Metrics render straight to the
  OpenMetrics text format through schema/value collection and
  `metered-om`. There is **no serde dependency** on the default path.

* **No magic, no globals.** Owner-local metrics, no shared `Arc` handles, no
  static state. Composition is explicit, via plain `struct`s, derives,
  `Registry`, or `MetricTreeView`.

## The model

Pick the layer that owns the information:

| You have… | Use… |
|---|---|
| A value your code owns and reads | readable metric state: concrete atomics implementing the `Counter` / `Gauge` traits, histograms, `Info`, `StateSet`, or a custom `Metric` |
| A dynamic label dimension | `Family<L, M>` -- one series per label set |
| Metrics inside a service graph | `MetricTreeView<C>` selectors or a borrowed `Registry` |
| Generic span lifecycle measurements | `metered-tracing` span name/kind/status metrics |
| Explicit non-tracing operation measurements | `metered::recording::Operation` behind the `recording` feature |
| Old method-level instrumentation | `legacy` feature compatibility APIs |

## Metrics as state

`Counter` and `Gauge` are traits implemented by concrete storage such as
standard-library atomics. The storage is the value your code uses, and it is
readable:

```rust
use metered::{Counter, Gauge};
use std::sync::atomic::AtomicU64;

let depth = AtomicU64::new(0);
Gauge::incr(&depth);         // the queue pushed
assert_eq!(Gauge::get(&depth), 1); // the queue reads its own depth here

let enabled = AtomicU64::new(0);
Gauge::set(&enabled, 1);
assert_eq!(Gauge::get(&enabled), 1);
```

(OpenMetrics has no boolean type, so a flag is the idiomatic gauge `0`/`1`. For
enum-like status use `StateSet`; for static build info use `Info`.)

Use gauges for current state, not event bookkeeping. A queue depth should usually
come from the queue (or a cached atomic updated during queue mutations), not from
a separate shadow metric that can drift.

## Existing state as metrics

Already have an `AtomicBool`, a queue length, or a config value? Expose it as a
metric without keeping a duplicate shadow value. The value is read when metrics
are collected:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use metered::adapter::flag;

let enabled = AtomicBool::new(true);
let metric = flag(|| enabled.load(Ordering::Relaxed)); // gauge 0/1, live
```

For service-level exposition, prefer a borrowed `MetricTreeView` or a
`#[derive(MetricTree)]` view struct. See the mdBook “Adding Metrics” guide for
the recommended service pattern.

Custom leaf metrics implement `Metric`; custom metric composites implement
`MetricTree`. Ready-made adapters like `adapter::flag`, `GaugeFn`, and
`CounterFn` are just convenience metric implementations for existing values.
For service-owned state, prefer a named newtype or a borrowed view struct when
that makes ownership and semantics clearer.

## Dynamic labels with `Family`

```rust
use metered::{Counter, Family};
use std::sync::atomic::AtomicU64;

let by_method: Family<Vec<(String, String)>, AtomicU64> = Family::with_label_names(["method"]);
by_method.with(&vec![("method".into(), "get".into())], |c| c.incr());
```

For typed, validated label sets, `#[derive(LabelSet)]` on a struct and use it as
the family key.

## Operation instrumentation

For RPC/HTTP services, prefer spans as the operation boundary. The generic
`metered-tracing` layer currently records span name, span kind, and final status
as metered counters and bucket histograms. Service/framework middleware can set
richer bounded attributes such as route, method, and error class for tracing
today, and for service-specific metric profiles built on top of metered later.

For explicit operation measurement without tracing, enable the `recording`
feature and use `metered::recording::Operation`:

```rust
use metered::recording::Operation;

let refresh = Operation::default();
let result = refresh.record(|| Ok::<_, &'static str>(()));
assert!(result.is_ok());
```

Legacy method wrappers and the `#[metered]` / `#[error_count]` macros remain
available behind the `legacy` feature for compatibility with older code. They
are not the default service instrumentation model.

## Exposing everything: `Registry`

The OpenMetrics text exposition lives in the separate `metered-om`
crate (the core `metered` crate is exposition-format-agnostic via its
`MetricSink` trait). Add both, then bring `OpenMetricsRegistryExt` into scope for
`encode_to_string`:

```rust
use metered::entry::{counter, gauge};
use metered::{Counter, Gauge, Registry, Unit};
use metered_om::OpenMetricsRegistryExt;
use std::sync::atomic::AtomicU64;

let requests = AtomicU64::new(0);
let queue_depth = AtomicU64::new(0);
Counter::incr(&requests);
Gauge::set(&queue_depth, 3);

let mut registry = Registry::with_prefix("myapp");
registry.label("env", "prod");
registry.register(counter("requests").source(&requests).help("Total requests handled"));
registry.register(gauge("queue_depth").source(&queue_depth).help("Items waiting").unit(Unit::Items));

let text = registry.encode_to_string().unwrap();
```

produces:

```text
# HELP myapp_requests Total requests handled
# TYPE myapp_requests counter
myapp_requests_total{env="prod"} 1
# HELP myapp_queue_depth Items waiting
# TYPE myapp_queue_depth gauge
# UNIT myapp_queue_depth items
myapp_queue_depth{env="prod"} 3
# EOF
```

Anything implementing `MetricTree` — a metric leaf, a `Family`, an `adapter`
metric, a `#[derive(MetricTree)]` struct, `metered::recording::Operation`, or a
legacy `#[metered]` registry — can be registered, and nested structures compose
through their schema/value collection implementations. `Registry::schema()`
returns the metric contract, `Registry::values()` samples the current values, and
`metered_om::OpenMetricsEncoder::encode_document(&schema, &values)`
renders both to text. Use `metered_om::OpenMetricsDocument::parse(...)`
to inspect emitted text structurally in tests or tooling instead of matching raw
strings.

If the metrics live inside an application context, use `MetricTreeView<C>` instead
of storing references in a `Registry`:

```rust
use metered::entry::{counter, gauge};
use metered::{MetricTreeView, Unit};
use std::sync::atomic::AtomicU64;

struct App {
    requests: AtomicU64,
    queue_depth: AtomicU64,
}

fn app_view() -> MetricTreeView<'static, App> {
    let mut view = MetricTreeView::with_prefix("app");
    view.register(
        counter("requests")
            .select(|app: &App| &app.requests)
            .help("Total requests")
            .unit(Unit::Requests),
    );
    view.register(
        gauge("queue_depth")
            .select(|app: &App| &app.queue_depth)
            .help("Queue depth")
            .unit(Unit::Items),
    );
    view
}
```

`MetricTreeView` stores only the selector closure. It does not own, clone, or share
the metric; each encode/schema call borrows through the supplied context.
For values that already live in the context but are not metric types, use the
closure helpers:

```rust
# use metered::entry::gauge_value;
# use metered::MetricTreeView;
# struct App { enabled: bool }
# let app = App { enabled: true };
# let mut view = MetricTreeView::with_prefix("myapp");
view.register(
    gauge_value("enabled")
        .read(|app: &App| app.enabled as i64)
        .help("Whether the app is enabled"),
);
```

## Histograms and exemplars

Metered duration measurements use cumulative, aggregatable histograms in seconds.
Today, `metered-tracing`, `metered::recording::Operation`, and legacy `Elapsed`
record into metered bucket histograms; exponential histogram backends are
available for explicit metric state and are the direction for future
tracing-profile integrations.

With the optional `exemplar-context` feature, a tracing layer can wire the active
trace's exemplar through a thread-local context. Legacy `Elapsed` can consume
that context when the `legacy` feature is enabled.

## Support crates

The workspace keeps integrations out of the core crate:

* `metered-om` renders OpenMetrics text, including VictoriaMetrics
  `vmrange` histogram rendering and Hyper 1 response helpers.
* `metered-tracing` turns `tracing` span lifecycle data into `MetricTree` values;
  enable its `exemplar` feature to feed trace/span IDs into OpenMetrics
  exemplars via `metered`'s `exemplar-context`.
* `metered-telemetry-tokio` exposes Tokio task/runtime telemetry as metric trees.
* `metered-om`'s `TextSourceTree` passes any classic Prometheus-text
  source (e.g. a metered 0.9 registry's serialized output) through to a 0.10
  scrape endpoint during migration.

## Feature flags

* `recording` *(off by default)* — `metered::recording::Operation` and
  `measure!` for explicit non-tracing operation measurement.
* `legacy` *(off by default)* — old method-level wrappers and `#[metered]` /
  `#[error_count]` compatibility APIs.
* `exemplar-context` *(off by default)* — an ambient thread-local exemplar
  context for wiring exemplars to the active trace.

## Stability & dependency policy

Metered is built so it — or parts of it — can be upgraded without dragging your
whole workspace along:

* **No foreign types in the default public API.** Bumping `metered` should not
  force unrelated dependency bumps on callers, and dependency types should not
  leak through metered's default signatures.
* **A single direct dependency.** The `LabelSet` / `MetricTree` derives are
  re-exported from `metered`. The legacy `#[metered]` / `#[error_count]`
  attribute macros are re-exported when the `legacy` feature is enabled, so you
  still depend on `metered` alone.
* **Hygienic, relocatable macros.** Generated code uses `::metered::` absolute
  paths, so it keeps working regardless of local items named `metered` or a
  renamed dependency.
* **An evolvable surface.** Open enums such as `MetricType` are
  `#[non_exhaustive]`, and extension traits stay minimal, so new OpenMetrics
  constructs can land without a breaking release. Internal sharing types (e.g.
  the `Arc` handle) are hidden behind opaque wrappers.
* **Toolchain-friendly.** The library does not `#![deny(warnings)]`, so a new
  compiler or clippy lint won't break your build until metered is patched (lint
  denial lives in CI instead).

## Extending

Implement `Metric` for a single leaf metric: it declares the OpenMetrics type
once and encodes the samples for that same family. Implement `MetricTree`
directly only for composite metric trees. With the `recording` or `legacy`
feature, implement `Measure` / `Recorder` to create explicit operation
instrumentation. Wrap a primitive in a newtype for domain-specific semantics.

## Required Rust version

Metered targets a recent stable Rust (uses, among others, `partition_point`,
`f64::total_cmp` and `const`-initialised thread locals). It does not require
nightly.

## Migrating

Code on `0.9.0` and earlier should read the mdBook section “Migrating from older
versions”. The short version: metrics are cumulative/source-of-truth values,
OpenMetrics is native, and the default path no longer relies on serde-based
exposition.

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

