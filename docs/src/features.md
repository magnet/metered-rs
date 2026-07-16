# Feature Flags and Stability

Metered's default build is lean and has no foreign types in its public API. Core
`metered` is metric state, composition, and schema/value collection; operation
instrumentation is layered through support crates or opt-in features.

## `metered` features

| Feature | Default | What it adds |
| --- | --- | --- |
| (none) | ✓ | The core model: readable metric state (`Counter` / `Gauge` implementors, histograms, `Info`, `StateSet`, `Family`), `Registry` / `MetricTreeView`, schema/value collection, `MetricTree` / `LabelSet` derives, name shaping. Use `metered-om` for text rendering, incremental rendering, parser support, and VictoriaMetrics `vmrange`. |
| `exemplar-context` | ✗ | `ThreadLocalExemplars` + `set_current_exemplar` / `with_exemplar`: an ambient exemplar seam for tracing layers (see [Exemplars](./exemplars.md)). |

## The `metered-semantic` crate

The method-level *semantic* model -- the `HitCount`, `ErrorCount`, `NoneCount`,
`InFlight` and `Elapsed` measuring wrappers, the `#[metered]` / `#[error_count]`
/ `measure!` macros, explicit recording, and the migration summary view -- lives
in the companion `metered-semantic` crate, which builds on the `metered` core.

| Feature | Default | What it adds |
| --- | --- | --- |
| (none) | ✓ | The semantic wrappers (`HitCount`, `ErrorCount`, `NoneCount`, `InFlight`, `Elapsed`) and the `#[metered]` / `#[error_count]` / `measure!` macros. |
| `recording` | ✗ | `metered_semantic::recording::Operation` for explicit non-tracing operation measurement: started, completed, failed, in-flight, and duration metrics. Prefer `metered-tracing` for RPC/HTTP/server operation measurements. |
| `migration` | ✗ | `LegacySummary` / `WithLegacySummary` / `SummaryWindow`: serve a legacy summary shape derived from a Metered histogram during a dashboard migration (see [Migrating](./migration.md)). |
| `exemplar-context` | ✗ | Forwards to `metered`'s `exemplar-context` so `Elapsed<ThreadLocalExemplars>` can wire exemplars to the active trace. |

Enable what you need:

```toml
[dependencies]
metered = "0.10"
metered-semantic = { version = "0.10", features = ["migration"] }
```

## Support crates

The support crates keep integration dependencies out of `metered` itself:

- `metered-om`: OpenMetrics text rendering, incremental rendering,
  snapshot caching, VictoriaMetrics `vmrange` rendering, and Hyper 1 helpers.
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

Metered is built so it -- or parts of it -- can be upgraded without dragging your
whole workspace along:

- **No foreign types in the default public API.** Neither `metered` nor
  `metered-semantic` pulls `serde` or `hdrhistogram` into the public API, so a
  `metered` bump never forces a `serde` or `hdrhistogram` bump on callers.
- **One direct dependency.** The procedural and derive macros are re-exported
  from `metered`, so downstream crates depend on `metered` alone (never
  `metered-macro`); the two halves always move together.
- **Hygienic, relocatable macros.** Generated code uses `::metered::` absolute
  paths, so it is immune to local name shadowing.
- **Evolvable surface.** Open enums such as `MetricType` are `#[non_exhaustive]`,
  so new OpenMetrics constructs can be added without a breaking change -- your
  `match`es just need a wildcard arm.

## Versioning

The crate is on the `0.10` line, heading toward a `1.0` that stabilizes the API.
Until then, minor releases may adjust unstable corners, but the principles above
(no leaked deps, single dependency, hygienic macros) are fixed commitments. If
you are coming from `0.9` or earlier, see
[Migrating From Older Versions](./migration.md).
