# Schema and dashboards

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
sibling extension traits) is a convenience over that schema/value/encode
pipeline. Because the core only exposes `schema()` / `values()` through the
`MetricSink` seam, a different exposition format is just a different sink crate.

For a single leaf metric, implement `Metric` instead. `Metric` couples the
OpenMetrics type and sample encoding in one place, and Metered provides the
`MetricTree` implementation from that single definition. Implement
`MetricTree` directly for composite trees that emit multiple families.

The schema captures:

- Family name.
- Metric type.
- `HELP` text.
- `UNIT`.
- Label names.

It also produces query seeds in Prometheus PromQL or in VictoriaMetrics
MetricsQL. The dialect chooses the histogram bucket grouping, `le` or
`vmrange`. It also chooses whether to wrap heatmaps in `prometheus_buckets`:

```rust
use metered::QueryDialect;

for query in schema.queries(QueryDialect::MetricsQl) {
    println!("{} => {}", query.title, query.expr);
}
```

These are not meant to replace a real dashboard author. They are a strong
starting point: counter rates, gauge/state panels, histogram p50/p95/p99, and a
heatmap seed with the right bucket grouping.

Some third-party values cannot implement `MetricTree`. For those, use
`Registry::register_opaque` or `Registry::register_opaque_with_unit`. Declare
the OpenMetrics type and label names explicitly. Pass a `collect` closure that
pushes the current samples into `MetricValues`. Prefer to implement
`MetricTree` when you own the type.

## Why the schema is separate

Because `describe` does not need a live scrape, the schema is available at
build time. You can generate documentation tables, dashboard templates, or
review-time diffs of what this service exposes. That works in CI, before
anything runs. And because the same tree defines `encode` as describe plus
collect, the schema you document and the metrics you emit cannot drift apart.