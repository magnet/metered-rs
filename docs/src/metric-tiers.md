# Two tiers: contract metrics vs diagnostics

Not all metrics are the same kind of thing. A team that treats them as one
bucket gets broken dashboards and 4 different latency metrics for the same
operation. The Metered model distinguishes two tiers, and serves each
differently.

## Tier 1 -- contract or platform "API" metrics

Some metrics are an **API**: a stable shape that many components expose
*identically*, that dashboards and alerts depend on, and that must not drift.
Examples:

- gRPC server metrics: request count, in-flight, latency histogram, error
  breakdown -- the same families for **every** service and method, distinguished
  only by `service` / `method` / `instance` labels.
- HTTP server metrics, the RED signals, the same for every route.
- Tokio runtime metrics: worker count, busy ratio, queue depths, poll counts.

These share three properties:

1. **Defined once, reused everywhere.** Every gRPC service should expose the
   *same* metric shape; you do not want each service inventing its own.
2. **A committed contract.** Renaming or reshaping them breaks fleet-wide
   dashboards and alerts, so they should not change casually.
3. **Populated by infrastructure, not business code.** A tower/tonic middleware
   or a runtime collector fills them in; the service author writes nothing.

### The contract is a type

The elegant part: in Metered, **the contract is just a typed `MetricTree`** --
no separate schema-assertion mechanism needed. An integration crate defines the
shape once:

```rust,ignore
// in metered-tonic (illustrative)
#[derive(Default, MetricTree)]
#[metrics(prefix = "rpc_server")]
pub struct GrpcServerMetrics {
    #[metrics(tree, rename = "requests")]
    requests: Family<MethodKey, std::sync::atomic::AtomicU64>,
    #[metrics(tree)]
    in_flight: Family<MethodKey, std::sync::atomic::AtomicI64>,
    #[metrics(tree)]
    duration_seconds: Family<MethodKey, BucketHistogram>,
    #[metrics(flatten)]
    errors: Family<MethodErrorKey, std::sync::atomic::AtomicU64>,
}
```

The middleware records into a `GrpcServerMetrics`. The middleware can only
record into *that* type, so the compiler enforces that every service emits
exactly the contract shape. There is nothing to keep in sync, no runtime schema
check, and no way to drift. The reusable struct *is* the contract, and
`describe()` is its machine-readable schema for docs and dashboards.

This is why name shaping (`#[metrics(rename/flatten)]`) matters here: the wire
contract stays fixed even as the integration crate refactors its internals.

## Tier 2 -- diagnostic, implementation-detail metrics

Other metrics are **implementation details**: service-specific counters and
timers you add to understand *this* code. Examples are a retry count, a cache
hit rate, and time spent in a specific phase. You expose them for
troubleshooting, and they evolve with the code. If one disappears in a refactor,
no fleet dashboard breaks.

These are exactly what owner-local primitives, `Family`, `MetricTree`, and
`MetricTreeView` are for: cheap to put next to the behavior, owner-local, no central
contract. For diagnostics on code that already carries `tracing` spans,
`metered-tracing` derives the metrics from those spans, on top of the same
primitives. Use name
shaping when you *want* a particular diagnostic to stay stable, but the default
expectation is that they track the code.

## Choosing the tier

| | Contract, Tier 1 | Diagnostic, Tier 2 |
| --- | --- | --- |
| Who defines the shape | an integration crate, once | the service author, as needed |
| Who populates it | middleware / collector | service code via primitives / families / views; span-derived via `metered-tracing` |
| Stability | committed; do not drift | tracks the code |
| Breaking it | breaks fleet dashboards | low stakes, troubleshooting only |
| Cardinality | bounded by label contract | bounded by author discipline |

A healthy service exposes **both**: the platform contract metrics, so it shows
up on the standard fleet dashboards for free, plus its own diagnostics. Metered
serves Tier 1 through integration crates that provide the reusable typed tree
and the collector. Examples are the runtime telemetry crates here -- Tokio,
process, and system -- or a framework's own RPC/HTTP contract trees built the
same way. Metered serves Tier 2 through owner-local primitives, families,
metric trees, and views. Both tiers encode into the same OpenMetrics document.
