# Feature flags and stability

Metered's default build is lean and has no foreign types in its public API. The
core crate, `metered-core`, is metric state, composition, and schema/value
collection. Operation instrumentation comes through support crates or opt-in
features.

## `metered-core` features

| Feature | Default | What it adds |
| --- | --- | --- |
| none | ✓ | The core model: readable metric state -- `Counter` / `Gauge` implementors, histograms, `Info`, `StateSet`, `Family` -- plus `Registry` / `MetricTreeView`, schema/value collection, `MetricTree` / `LabelSet` derives, and name shaping. Use `metered-om` for text rendering, incremental rendering, parser support, and VictoriaMetrics `vmrange`. |
| `exemplar-context` | ✗ | `ThreadLocalExemplars` + `set_current_exemplar` / `with_exemplar`: an ambient exemplar seam for tracing layers. See [Exemplars](./exemplars.md). |

## The `metered` facade

`metered` is a facade crate. It re-exports the whole core model,
`metered-core`, wholesale. It surfaces the support crates as feature-gated
modules, so apps carry one dependency:

| Feature | Default | What it adds |
| --- | --- | --- |
| `om` | ✗ | `metered::om`: OpenMetrics text exposition from `metered-om`. |
| `tracing` | ✗ | `metered::tracing`: tracing-subscriber layers from `metered-tracing`. |
| `telemetry-tokio` | ✗ | `metered::telemetry_tokio`: Tokio runtime/task telemetry from `metered-telemetry-tokio`. |
| `telemetry-process` | ✗ | `metered::telemetry_process`: process telemetry from `metered-telemetry-process`. |
| `telemetry-system` | ✗ | `metered::telemetry_system`: host system telemetry from `metered-telemetry-system`. Raises the required `rustc` to 1.95 for `sysinfo`. Every other crate and feature holds at 1.85. |
| `exemplar-context` | ✗ | Forwards `metered-core/exemplar-context`. |
| `full` | ✗ | All of the preceding features. |

Libraries that want maximal stability can depend on `metered-core` directly.
The facade re-exports the same types, so their metric trees compose into any
app.

## Support crates

The support crates keep integration dependencies out of `metered` itself:

- `metered-om`: OpenMetrics text rendering, incremental rendering,
  snapshot caching, VictoriaMetrics `vmrange` rendering, and Hyper 1 helpers
  (behind its `hyper-1` feature).
- `metered-tracing`: `tracing-subscriber` layers that turn spans into
  *semantic* metric families (one `SpanMetric` per span kind), each a counter +
  duration histogram labeled from the span's fields. Optional `exemplar` feature
  feeds trace/span IDs into `metered`'s ambient exemplar context.
- `metered-telemetry-tokio`: Tokio task/runtime telemetry as metric trees.
- `metered-telemetry-process`: standard process telemetry (CPU, memory, file
  descriptors, threads) under the canonical `process_*` names, sampled
  cross-platform on each scrape.
- `metered-telemetry-system`: host telemetry (CPU, memory, swap, load average,
  uptime) as a metric tree; optional `tokio` feature samples on a background task
  so the scrape path stays non-blocking.

## Stability and dependency policy

Metered lets you upgrade it, or parts of it, without an upgrade of your whole
workspace:

- **No foreign types in the default public API.** `metered` pulls neither
  `serde` nor `hdrhistogram` into its public API, so a `metered` bump never
  forces a `serde` or `hdrhistogram` bump on callers.
- **One direct dependency.** The procedural and derive macros are re-exported
  from `metered`, so downstream crates depend on `metered` alone (never
  `metered-macro`); the two halves always move together.
- **Hygienic, relocatable macros.** Generated code uses `::metered::` absolute
  paths, so it is immune to local name shadowing.
- **Evolvable surface.** Open enums such as `MetricType` are `#[non_exhaustive]`,
  so new OpenMetrics constructs can land without a breaking change -- your
  `match`es just need a wildcard arm.

## Versioning

The crate is on the `0.10` line, heading toward a `1.0` that stabilizes the API.
Until then, minor releases may adjust unstable corners. The preceding
principles -- no leaked dependencies, a single dependency, hygienic macros --
do not change. If
you are coming from `0.9` or earlier, see
[Migrating From Older Versions](./migration.md).
