# Migrating from older versions

This section is for users migrating from `0.9.0` and earlier.

Metered 0.10 is a **new metric model**, not a new version of the old API. There
is no source-compatibility shim. The new model removes the method-level
measuring macros and wrappers. It removes `Clear`: metrics are cumulative or
source-of-truth state. It moves `serde` off the default path. Cumulative bucket
and exponential histograms replace the HDR `ResponseTime`/`Throughput`
summaries, and you compute their quantiles at query time.

## The strategy: coexistence, not conversion

You do not port a service in one commit. Cargo treats `metered` 0.9 and 0.10 as
**distinct packages**, so both can live in one binary while you migrate. Keep
old modules on 0.9 through a renamed dependency, and let new and migrated code
use 0.10:

```toml
[dependencies]
metered = "0.10.0-rc.1"
metered09 = { package = "metered", version = "0.9" }
```

Old code changes only its `use` paths, for example `use metered09::...`. Its metrics keep
recording exactly as before. Migrate module by module. After you migrate the
last 0.9 metric, delete `metered09` and the bridge below.

## One endpoint from day one: bridge the 0.9 metrics

Exposition unifies on the 0.10 side: a single OpenMetrics endpoint, served by a
0.10 `Registry` / `MetricTreeView`, carries both worlds. The 0.9 registries do
not implement 0.10's `MetricTree`, so you write a small **bridge**. The bridge
is a hand-written `MetricTree` `impl` that holds the 0.9 registry/metric
handles. It reads their current values at collect time and re-emits them
through the 0.10 schema/values API.

The bridge is a recipe, not a shipped crate. You own it, and it is a few dozen
lines. You delete it at the end of the migration. The sketch that follows is
illustrative, not compiled, because 0.9 is not a dependency of this workspace:

```rust,ignore
use metered::{join_name, MetricSchema, MetricTree, MetricType, MetricValues};
use std::sync::Arc;

/// Bridges still-live 0.9 metrics into the 0.10 exposition.
struct Bridge09 {
    /// Your macro-generated 0.9 registry for the orders module.
    orders: Arc<OrderServiceMetrics>,
}

impl MetricTree for Bridge09 {
    fn describe(&self, name: &str, labels: &[(&str, &str)], schema: &mut MetricSchema) {
        schema.add_family(&join_name(name, "find_order_hits"), MetricType::Counter, labels);
    }

    fn collect(&self, name: &str, labels: &[(&str, &str)], values: &mut MetricValues) {
        // Read the 0.9 hit counter's current value (`.0` is its inner
        // `AtomicInt`) and re-emit it as a 0.10 counter sample --
        // `values.counter` adds the `_total` suffix.
        let hits = self.orders.find_order.hit_count.0.get();
        values.counter(&join_name(name, "find_order_hits"), labels, hits);
    }
}
```

Mount the bridge in your 0.10 registry or view like any other tree. Because
you write `describe` and `collect` together, the bridged families get real
`# TYPE` lines, prefixes, and constant labels. They are first-class 0.10
metrics whose *storage* happens to still be in the 0.9 crate. As each module
migrates to native 0.10 state, delete its lines from the bridge.

If you would rather not name each metric, `metered_om::TextSourceTree` is the
zero-effort alternative. It re-encodes a 0.9 registry's serialized
`serde_prometheus` output through the 0.10 endpoint. The re-encode
normalizes. Names, labels, and shapes survive, so an HDR summary stays a
summary. Value tokens re-encode from their parsed form, and the re-encode
drops sample timestamps.

It gives you no schema, no type checking, and no name shaping. Use it as a
stopgap, and use the bridge as the managed path.

## Dashboards

A 0.9 HDR summary exposed pre-computed quantiles, for example
`name{quantile="0.99"}`. A
0.10 histogram exposes `_bucket`/`_sum`/`_count`, and dashboards query
`histogram_quantile(0.99, ...)` instead. Update the panels for a metric when
you migrate its module -- `Registry::schema()` and the
[Schema and Dashboards](./schema-dashboards.md) section generate the starting
queries. Query-time quantiles aggregate correctly across replicas, which the
pre-computed ones never did.

## The endgame

The migration ends when `metered09` disappears from `Cargo.toml` and you
delete the bridge type. There is nothing else to unwind: the endpoint, names,
and dashboards were on the 0.10 shape all along.
