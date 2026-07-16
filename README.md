# metered-rs

[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](https://github.com/magnet/metered-rs)
[![Cargo](https://img.shields.io/crates/v/metered.svg)](https://crates.io/crates/metered)
[![Documentation](https://docs.rs/metered/badge.svg)](https://docs.rs/metered)
[![Book](https://img.shields.io/badge/book-Metered-blue.svg)](https://magnet.github.io/metered-rs/)

## Fast, ergonomic, OpenMetrics-native metrics for Rust

Metered helps you measure your programs in production. It is a metric-state,
composition, and schema/value collection library for Rust services. Exposition
formats live in sink crates such as `metered-om`.

**[The Metered book](https://magnet.github.io/metered-rs/)** is the full
guide. It covers the metric model, every metric type and when to use it,
labels, name shaping, exemplars, tracing, dashboards, and migration. This
README is the tour. The book is the reference.

Core `metered` gives services readable metric state (`Counter`, `Gauge`,
histograms, `Family`), typed metric trees, and schema/value collection. It also
gives registry views that borrow through the real service graph at scrape time.

Metered layers operation instrumentation:

- Does your code already emit [`tracing`](https://docs.rs/tracing) spans?
  `metered-tracing` turns them into metric families. An instrumented method
  gets counters and duration histograms from the span it already opens. You
  instrument once and pay once, for both traces and metrics.
- Everything else is plain core metric state -- counters, gauges, and
  histograms your code owns and updates directly.

A few ideas guide Metered:

* **Metrics are state, not shadows.** A metric is the live value your code uses:
  an enabled flag can be a gauge-valued atomic that the service reads, and a
  queue-depth gauge should come from the queue or its cached depth. Metrics are
  readable, not write-only.

* **Native OpenMetrics, zero serialization.** Metrics encode straight to the
  OpenMetrics text format through schema/value collection and
  `metered-om`. There is **no `serde` dependency** on the default path.

* **No magic, no globals.** Owner-local metrics, no shared `Arc` handles, no
  static state. Composition is explicit, via plain `struct`s, derives,
  `Registry`, or `MetricTreeView`.

## Getting started

Applications depend on the `metered` facade and enable the satellites they
need as features:

```toml
[dependencies]
metered = { version = "0.10.0-rc.1", features = ["om"] }
```

Library authors who only produce metrics can depend on `metered-core` directly
for a maximally stable, sink-free surface. The facade re-exports the same
types, so their trees compose into any app.

## The model

Pick the layer that owns the information:

| You have… | Use… |
|---|---|
| A value your code owns and reads | readable metric state: concrete atomics implementing the `Counter` / `Gauge` traits, histograms, `Info`, `StateSet`, or a custom `Metric` |
| A dynamic label dimension | `Family<L, M>` -- one series per label set |
| Metrics inside a service graph | `MetricTreeView<C>` selectors or a borrowed `Registry` |
| Generic span lifecycle measurements | `metered-tracing` span name/kind/status metrics |
| An operation boundary without spans | plain core state: a counter plus a duration histogram through `BucketHistogram::observe_duration` |

## Metrics as state

`Counter` and `Gauge` are the contracts the exposition side reads through.
Standard-library atomics already implement them, so the atomic your code
updates *is* the metric. Write your code like you want. There is no metrics
API to call on the hot path:

```rust
use std::sync::atomic::{AtomicU64, Ordering};

let depth = AtomicU64::new(0);
depth.fetch_add(1, Ordering::Relaxed);        // the queue pushed
assert_eq!(depth.load(Ordering::Relaxed), 1); // the queue reads its own depth

let enabled = AtomicU64::new(1); // a flag is the idiomatic 0/1 gauge
```

OpenMetrics has no boolean type, so a flag is the idiomatic gauge `0`/`1`. For
enum-like status, use `StateSet`. For static build info, use `Info`.

Use gauges for current state, not event bookkeeping. A queue depth should usually
come from the queue, or from a cached atomic that the queue updates during
mutations. Do not use a separate shadow metric that can drift.

## Existing state as metrics

Already have an `AtomicBool`, a queue length, or a configuration value? Expose
it as a metric without keeping a duplicate shadow value. Metered reads the value
at collection time:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use metered::adapter::flag;

let enabled = AtomicBool::new(true);
let metric = flag(|| enabled.load(Ordering::Relaxed)); // gauge 0/1, live
```

For service-level exposition, prefer a borrowed `MetricTreeView` or a
`#[derive(MetricTree)]` view struct. See the book’s [adding metrics guide](https://magnet.github.io/metered-rs/adding-metrics.html) for
the recommended service pattern.

Custom leaf metrics implement `Metric`. Custom metric composites implement
`MetricTree`. Ready-made adapters like `adapter::flag`, `GaugeFn`, and
`CounterFn` are just convenience metric implementations for existing values.
Prefer a named newtype or a borrowed view struct for service-owned state when
that makes ownership and semantics clearer.

## Dynamic labels with `Family`

```rust
use metered::{Counter, Family};
use std::sync::atomic::AtomicU64;

let by_method: Family<Vec<(String, String)>, AtomicU64> = Family::with_label_names(["method"]);
by_method.with(&vec![("method".into(), "get".into())], |c| c.incr());
```

For typed, validated label sets, `#[derive(LabelSet)]` on a struct and use it as
the family key. When your state is already a keyed map of components, expose it
borrowed as a [family view](https://magnet.github.io/metered-rs/labels-families.html)
instead of mirroring it into an owned `Family`. Use `family_view`, or its
one-string-key sugar `family_by`.

## Operation instrumentation

If an operation already runs inside a `tracing` span -- an
`#[tracing::instrument]`-ed method, or a span your middleware opens --
`metered-tracing` derives its metrics from that span. The layer records span
name, span kind, and final status as Metered counters and bucket histograms.
One instrumentation produces both the trace and the metrics. The code never
pays twice for the same measurement.

Server middleware for RPC/HTTP golden signals is framework territory. Build it
on plain core state, such as counters, duration histograms, and `Family`, like
any other component that owns what it measures. When such middleware already
opens semconv spans, it can derive the metrics from them through
`metered-tracing`. The order-service example shows that pattern.

For an operation that is not span-shaped, use plain core state. Examples are a
background refresh and a batch step. Use a counter for attempts, a counter for
failures, and a `BucketHistogram` with `observe_duration`. The metrics live on
the component that owns the operation, like any other state.

## Exposing everything: `Registry`

The OpenMetrics text exposition lives in the separate `metered-om`
crate (the core `metered` crate is exposition-format-agnostic via its
`MetricSink` trait). Add both, then bring `OpenMetricsRegistryExt` into scope for
`encode_to_string`:

```rust
use metered::entry::{counter, gauge};
use metered::{Registry, Unit};
use metered_om::OpenMetricsRegistryExt;
use std::sync::atomic::{AtomicU64, Ordering};

let requests = AtomicU64::new(0);
let queue_depth = AtomicU64::new(0);
requests.fetch_add(1, Ordering::Relaxed);
queue_depth.store(3, Ordering::Relaxed);

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

You can register anything that implements `MetricTree`: a metric leaf, a
`Family`, an `adapter` metric, or a `#[derive(MetricTree)]` struct. Nested
structures compose
through their schema/value collection implementations. `Registry::schema()`
returns the metric contract, `Registry::values()` samples the current values, and
`metered_om::OpenMetricsEncoder::encode_document(&schema, &values)`
renders both to text. Use `metered_om::OpenMetricsDocument::parse(...)`
to inspect emitted text structurally in tests or tooling instead of matching raw
strings.

If the metrics live inside an app context, use `MetricTreeView<C>` instead
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
the metric. Each encode/schema call borrows through the supplied context.
For values that already live in the context but are not metric types, use the
closure helpers:

```rust
use metered::entry::gauge_value;
use metered::MetricTreeView;

struct App {
    enabled: bool,
}

let mut view = MetricTreeView::with_prefix("myapp");
view.register(
    gauge_value("enabled")
        .read(|app: &App| app.enabled as i64)
        .help("Whether the app is enabled"),
);
```

## Histograms: pick your distribution engine

Metered ships four histogram instruments and a summary. All duration
measurements are cumulative histograms in seconds, so backends can aggregate
them across replicas. The book's
[histograms in depth section](https://magnet.github.io/metered-rs/histograms.html)
carries the full comparison: who chooses the buckets, memory, observe cost,
and saturation behavior. The short version:

| Instrument | Buckets | Memory | Observe path | Reach for it when |
| --- | --- | --- | --- | --- |
| `BucketHistogram` | hand-picked `le` bounds | fixed | lock-free | you know the boundaries that matter, such as your latency-objective edges |
| `FixedExponentialHistogram` | implicit powers of `2^(2^-schema)`, dense | fixed by the configured range | fully lock-free | one hot histogram over a known range |
| `DynamicExponentialHistogram` | implicit, **sparse** | tracks the buckets you hit | lock-free: one `log2`, one atomic add | wide or unknown ranges, many instances -- the default |
| `GaugeBuckets` | hand-picked, current population | fixed | lock-free `enter`/`leave` | distributions that go down, such as the sizes of items held now |
| `Summary` | pre-computed quantiles over a `QuantileSource` | reads your distribution | -- | exact per-instance quantiles, legacy dashboard parity |

**The star is `DynamicExponentialHistogram`.** One parameter, the `schema`,
gives bounded *relative* error across the whole value range: `schema` 3 is
about 9% per bucket, 5 is 2.2%, 8 is 0.27%. There is no boundary list to
choose, tune, or migrate. Memory tracks the buckets your values populate, not
the range you configured, so a fleet of them stays cheap. The observe path is
one `log2` and one atomic increment.

When the bucket table saturates, the histogram merges adjacent buckets to make
room. That rebuild runs off the hot path, at scrape time. A lock-free swap
publishes the coarser table. Observers never block on it, and a straggler's
increment is never dropped. Exemplars ride on the buckets, also lock-free.

An exponential histogram can encode as classic `le` buckets or as
VictoriaMetrics `vmrange` series. The family declares its intent and the sink
resolves it. See
[the OpenMetrics section](https://magnet.github.io/metered-rs/openmetrics.html).

## Exemplars

With the optional `exemplar-context` feature, a tracing layer can wire the
active trace's exemplar through a thread-local context. Read it with
`ThreadLocalExemplars`. Attach it to an observation with
`BucketHistogram::observe_with_exemplar`.

## Support crates

The workspace keeps integrations out of the core crate:

* `metered-om` renders OpenMetrics text, including VictoriaMetrics
  `vmrange` histogram rendering and Hyper 1 response helpers.
* `metered-tracing` turns `tracing` span lifecycle data into `MetricTree` values;
  enable its `exemplar` feature to feed trace/span IDs into OpenMetrics
  exemplars via `metered`'s `exemplar-context`.
* `metered-telemetry-tokio` exposes Tokio task/runtime telemetry as metric trees.
* `metered-telemetry-process` exposes standard process telemetry (CPU, memory,
  file descriptors) under the canonical `process_*` names.
* `metered-telemetry-system` exposes host telemetry (CPU, memory, swap, load,
  uptime) as a metric tree.
* `metered-om`'s `TextSourceTree` passes any classic Prometheus-text
  source through to a 0.10 scrape endpoint during migration. An example source
  is the serialized output of a `metered` 0.9 registry.

## Feature flags

The `metered` facade re-exports the support crates behind features: `om`,
`tracing`, `telemetry-tokio`, `telemetry-process`, `telemetry-system`, and
`full` for everything at once.

The core model has a single optional feature, forwarded by the facade:

* `exemplar-context` *(off by default)*: an ambient thread-local exemplar
  context that wires exemplars to the active trace. `metered-tracing`'s
  `exemplar` feature builds on it.

## Stability & dependency policy

Metered lets you upgrade it, or parts of it, without an upgrade of your whole
workspace:

* **No foreign types in the default public API.** A `metered` version bump
  should not force unrelated dependency bumps on callers. Dependency types
  should not leak through the default signatures of `metered`.
* **A single direct dependency.** The `LabelSet` / `MetricTree` derives are
  re-exported from `metered`, so you never depend on `metered-macro` directly.
* **Hygienic, relocatable macros.** Generated code uses `::metered::` absolute
  paths, so it keeps working regardless of local items named `metered` or a
  renamed dependency.
* **An evolvable surface.** Open enums such as `MetricType` are
  `#[non_exhaustive]`, and extension traits stay minimal, so new OpenMetrics
  constructs can land without a breaking release. Opaque wrappers hide internal
  sharing types, for example the `Arc` handle.
* **Toolchain-friendly.** The library does not `#![deny(warnings)]`, so a new
  compiler or `clippy` lint won't break your build until the maintainers patch
  `metered`. Lint denial lives in CI instead.

## Extending

Implement `Metric` for a single leaf metric: it declares the OpenMetrics type
once and encodes the samples for that same family. Implement `MetricTree`
directly only for composite metric trees. Wrap a primitive in a newtype for
domain-specific semantics.

## Required Rust version

Every crate sets `rust-version = "1.85"`, except `metered-telemetry-system`,
which needs 1.95 for its `sysinfo` backend. CI compiles both floors on the real
toolchains. Nightly is not required.

## Migrating

Code on `0.9.0` and earlier should read the book’s [migration section](https://magnet.github.io/metered-rs/migration.html) “Migrating from older
versions”. The short version: 0.10 is a new metric model with no
source-compatibility shim, so migration is coexistence. Keep old modules on a
renamed `metered` 0.9 dependency, and move code module by module. Serve one
OpenMetrics endpoint from the 0.10 side, and bridge still-live 0.9 metrics into
it, until no 0.9 metric remains.

## License

Licensed under either of

* Apache License, Version 2.0: [LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0
* MIT license: [LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT

at your option.

### Contribution

Unless you explicitly state otherwise, the preceding dual license applies to
any contribution that you intentionally submit for inclusion in the work. The
Apache-2.0 license defines intentional submission. No extra terms or conditions
apply.

