# Migrating from older versions

This section is for users migrating from `0.9.0` and earlier.

- `Clear` is removed. Metrics are cumulative or source-of-truth state.
- `serde` is not on the default path.
- HDR `ResponseTime`/`Throughput` are removed. To serve a 0.9 registry's
  serialized output on a 0.10 scrape endpoint, mount it with
  `metered_om::TextSourceTree` (a generic Prometheus-text source — pass
  a closure that produces the 0.9 registry's `serde_prometheus` output); to run
  0.9 in-process, depend on the published `metered` 0.9 crate.
- Prefer Metered's cumulative bucket or exponential histograms for aggregatable
  latency. If you are migrating method-level instrumentation, `Elapsed` and the
  other semantic wrappers live in the `metered-semantic` crate.
- Prefer `Registry` + `MetricTree` for exposition and schema.
- Use `Registry::schema()` to generate documentation and dashboard starting
  points.

For old code that cleared metrics between pushes, move to pull-based cumulative
OpenMetrics. Prometheus/VictoriaMetrics should compute rates and quantiles at
query time.

## Migration mode: serving both shapes at once

Switching a latency metric from a summary (pre-computed quantiles) to a native
`Histogram` changes its exposition shape, which breaks any dashboard panel that
queries `name{quantile="..."}`. The `metered-semantic` crate's `migration`
feature lets you serve **both shapes from the same recorded data** while you
migrate those panels, then remove it.

Enable it in `Cargo.toml`:

```toml
[dependencies]
metered = "0.10"
metered-semantic = { version = "0.10", features = ["migration"] }
```

Keep your instrumentation exactly as it is (one `BucketHistogram` or other
`Histogram` backend). At exposition time, register the histogram under its new
name and a `migration::LegacySummary` -- which reads quantiles off the very same
histogram buckets -- under the old name:

```rust
use metered::entry::metric;
use metered::{BucketHistogram, Registry};
use metered_semantic::migration::LegacySummary;
use metered_om::OpenMetricsRegistryExt;

# let latency = BucketHistogram::default();
let legacy = LegacySummary::new(&latency);

let mut registry = Registry::new();
registry.register(metric("http_request_duration_seconds").source(&latency).help("Request latency"));
registry.register(metric("response_time").source(&legacy).help("Legacy latency summary"));

let text = registry.encode_to_string().unwrap();
```

`LegacySummary` defaults to the legacy quantiles (`0.9, 0.95, 0.99, 0.999`); use
`with_quantiles(...)` to change them. If old tooling expects a *clearable*
summary, pair it with a `SummaryWindow`: `LegacySummary::windowed(&latency,
&window)` reports values since `window.clear(&latency)` was last called, without
disturbing the cumulative histogram. In `metered-semantic`, `Elapsed` is a measuring wrapper rather
than a histogram backend, so use `LegacySummary::from_elapsed(&elapsed)` or
`LegacySummary::windowed_elapsed(&elapsed, &window)` when migrating an existing
`Elapsed` field by hand.

### One registration with `WithLegacySummary`

To avoid registering the summary by hand, hold a `WithLegacySummary` in place of
the histogram. It derefs to the inner metric (so `observe` / `measure!` /
`#[metered]` usage is unchanged) and emits both shapes from a single
registration:

```rust
use metered::entry::metric;
use metered::{BucketHistogram, Registry};
use metered_semantic::migration::WithLegacySummary;

// Was `latency: BucketHistogram`.
let latency = WithLegacySummary::new(BucketHistogram::default(), "response_time");
latency.observe(0.012); // unchanged instrumentation, via Deref

let mut registry = Registry::new();
registry.register(metric("http_request_duration_seconds").source(&latency).help("Request latency"));
// `response_time` (summary) is emitted too, from the same data.
```

`legacy_help(...)`, `with_quantiles(...)`, and `windowed()` (plus
`clear_window()` / `reset_window()`) configure the legacy view. The legacy name
is absolute -- a `Registry` prefix applies to the Metered histogram metric but
not to it, so pass the exact legacy series name your dashboards query.

This is intentionally temporary. Summaries do not aggregate across replicas and
the quantiles are bucket-resolution estimates. Once the dashboards point at the
histogram, drop the `migration` feature and the legacy registration.
