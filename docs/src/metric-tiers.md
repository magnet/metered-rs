# Two Tiers: Contract Metrics vs Diagnostics

Not all metrics are the same kind of thing. Treating them as one bucket is why
"metrics break dashboards" and "we have 4 different latency metrics" happen.
Metered's model distinguishes two tiers, and serves each differently.

## Tier 1 -- Contract (platform / "API") metrics

Some metrics are an **API**: a stable shape that many components expose
*identically*, that dashboards and alerts are built on, and that must not drift.
Examples:

- gRPC server metrics: request count, in-flight, latency histogram, error
  breakdown -- the same families for **every** service and method, distinguished
  only by `service` / `method` / `instance` labels.
- HTTP server metrics (the RED signals), the same for every route.
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

The middleware records into a `GrpcServerMetrics`. Because the middleware can only
record into *that* type, the compiler guarantees every service emits exactly the
contract shape -- there is nothing to keep in sync, no runtime schema check, no
way to drift. The reusable struct *is* the contract, and `describe()` is its
machine-readable schema (for docs and dashboards).

This is why name shaping (`#[metrics(rename/flatten)]`) matters here: the wire
contract stays fixed even as the integration crate refactors its internals.

## Tier 2 -- Diagnostic (implementation-detail) metrics

Other metrics are **implementation details**: service-specific counters and
timers you add to understand *this* code -- a retry count, a cache hit rate, time
spent in a specific phase. They are exposed for troubleshooting, they evolve with
the code, and if one disappears in a refactor, no fleet dashboard breaks.

These are exactly what owner-local primitives, `Family`, `MetricTree`, and
`MetricTreeView` are for: cheap to put next to the behavior, owner-local, no central
contract. With the `metered-semantic` crate, `#[metered]` / `measure!` can
generate method-level diagnostics, but that is an opt-in layer rather than the
default service instrumentation path. Use name shaping when you *want* a particular
diagnostic to stay stable, but the default expectation is that they track the
code.

## Choosing the tier

| | Contract (Tier 1) | Diagnostic (Tier 2) |
| --- | --- | --- |
| Who defines the shape | an integration crate, once | the service author, ad hoc |
| Who populates it | middleware / collector | service code via primitives / families / views; `metered-semantic` `#[metered]` when added |
| Stability | committed; do not drift | tracks the code |
| Breaking it | breaks fleet dashboards | low stakes (troubleshooting) |
| Cardinality | bounded by label contract | bounded by author discipline |

A healthy service exposes **both**: the platform contract metrics (so it shows up
on the standard fleet dashboards for free) plus its own diagnostics. Metered
serves Tier 1 through integration crates (gRPC/HTTP/tokio) that provide the
reusable typed tree and the collector, and Tier 2 through owner-local primitives,
families, metric trees, and views. Both render into the same OpenMetrics
document.
