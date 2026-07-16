# Schema and Dashboards

Every metric tree can both describe its schema and collect current values. The
registry accepts `MetricTree` values and exposes both halves separately:

```rust
let schema = registry.schema();
let values = registry.values();
```

A sink combines those two parts. The OpenMetrics text sink lives in the
`metered-om` crate:

```rust
use metered_om::OpenMetricsEncoder;

let mut text = String::new();
let mut encoder = OpenMetricsEncoder::new(&mut text);
encoder.encode_document(&schema, &values).unwrap();
encoder.finish().unwrap();
```

`encode_to_string()` (from `metered_om::OpenMetricsRegistryExt` and the
sibling extension traits) is a convenience over that schema/value/render
pipeline. Because the core only exposes `schema()` / `values()` through the
`MetricSink` seam, a different exposition format is just a different sink crate.

For a single leaf metric, implement `Metric` instead. `Metric` couples the
OpenMetrics type and sample encoding in one place, and metered provides the
`MetricTree` implementation from that single source of truth. Implement
`MetricTree` directly for composite trees that emit multiple families.

The schema captures:

- Family name.
- Metric type.
- HELP text.
- UNIT.
- Label names.

It also produces query seeds, in either Prometheus PromQL or VictoriaMetrics
MetricsQL (the dialect chooses the histogram bucket grouping -- `le` vs
`vmrange` -- and whether heatmaps are wrapped in `prometheus_buckets`):

```rust
use metered::QueryDialect;

for query in schema.queries(QueryDialect::MetricsQl) {
    println!("{} => {}", query.title, query.expr);
}
```

These are not meant to replace a real dashboard author. They are a strong
starting point: counter rates, gauge/state panels, histogram p50/p95/p99, and a
heatmap seed with the right bucket grouping.

For third-party values you cannot make implement `MetricTree`, use
`Registry::register_opaque` or `Registry::register_opaque_with_unit`: declare the
OpenMetrics type and label names explicitly, and pass a `collect` closure that
pushes the current samples into `MetricValues`. Prefer implementing `MetricTree`
when you own the type.

## Why the schema is separate

Because `describe` does not need a live scrape, the schema is a build-time-ish
artifact: you can generate documentation tables, dashboard templates, or
review-time diffs of "what does this service expose?" from it -- in CI, before
anything runs. And because the same tree's `encode` is defined as describe +
collect, the schema you document and the metrics you emit cannot drift apart.