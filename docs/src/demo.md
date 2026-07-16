# Demo App

The demo in `examples/order-service` is a small e-commerce service that shows how
Metered fits a real service: spans become metrics, components own their metric
layout, and a dynamic fleet of payment rails is fanned out by label. Treat it as
the executable companion to this book.

```bash
cargo run -p order-service-demo
```

It runs a 100-request workload with the span-metrics layer installed, then prints
the OpenMetrics exposition for the whole service.

## The modules

| Module | Pattern it teaches |
| --- | --- |
| `app` | the composition root: a custom `ServiceIdentity` implementing `Info`, and `App` deriving `MetricTree` to mount components in two `flatten`ed groups -- `Standard` (shared names + `service` label) and `Business` (`order_service_` prefix, no label). The routing layer is assembled here from the components' own span metrics |
| `telemetry` | the cross-cutting policy: the span (dotted semconv) ↔ metric (snake OpenMetrics) naming translation and the shared exemplar provider. There is **no** telemetry bundle -- components own their span metrics |
| `rpc` | a Tower-style metrics layer that *owns* its `rpc.server` `SpanMetric` but never records by hand: it only opens an `rpc.server` semconv span (and mints trace context); the metrics are derived from the span |
| `orders` | business counters via a typed `Family` + `#[derive(LabelSet)]`, the order cache exposed as a computed gauge, and the `orders.create_order` operation span |
| `db` | total schema control via a **view over real internals**: the connection pool's raw atomics shaped into chosen names/types, plus a synthesized `pool_utilization` no field stores, alongside `db.query` span-derived latency |
| `jobs` | a real job runner: a live queue exposed as a gauge, plus a `jobs.run` span with an `outcome` label |
| `payments` | a **dynamic** sub-service fleet of payment rails: `each` walks a live map and emits every rail's own metrics labeled by name (a rail is onboarded at runtime) |

## What to notice

The demo varies along two orthogonal axes; keep them separate while reading:

- **Composition shape** -- how a metric is spliced into the document:
  `subtree` (a child mounted under a name segment), `flatten` (splice a sub-tree
  with no segment), `each` (N dynamic members keyed by a label).
- **Where the numbers come from** -- how a metric is backed: a pure-metrics
  struct + `#[derive(MetricTree)]`, a hand-written view over real internals, or
  span-derived via `SpanMetric`.

Most components mix several (e.g. `orders` does all three of the second axis).

- **Components own their metrics.** Each component implements `MetricsView`;
  the app never restates a child's metrics, it just splices them. New
  subsystem, one line.
- **Two naming conventions, one translation.** Spans use OTel semconv (dotted:
  `rpc.method`, `order.category`); metrics use OpenMetrics (snake: `rpc_method`).
  The `SpanMetric` translates between them, labeling a curated low-cardinality
  subset; the rest stays span-only as trace/exporter context.
- **Spans are the metrics.** The RPC layer, DB, orders, and jobs only open
  semconv spans; a `metered-tracing` layer turns them into `rpc_server_*`,
  `db_client_*`, `orders_create_*`, and `jobs_run_*` families, each labeled from
  span fields. Each component **owns** its `SpanMetric` and mounts it in its own
  view; the service assembles the routing layer from those handles with
  `.source(&component)`. There is no telemetry blob to flatten.
- **Exemplars link metrics to traces.** The RPC layer puts `trace_id`/`span_id`
  on its span; `rpc_server_duration_seconds` buckets carry the matching exemplar.
- **Derive for pure metrics, view for real internals.** `BusinessMetrics` *is*
  metrics, so it uses `#[derive(MetricTree)]`. The DB connection pool is real
  operational state with no place for `#[metrics]` attributes, so its schema is a
  hand-shaped **view** over the raw atomics -- which is also where you get total
  control: chosen names, gauge-vs-counter, and synthesized metrics like
  `pool_utilization` that no field holds.
- **Dynamic shape, not a central Family.** `payments` keeps `in_flight` /
  `settlements` / `failures` *inside* each `PaymentRail` (the service's real
  shape) and an `each` view fans them out as
  `order_service_payments_settlements_total{rail="card"}` over a live,
  dynamic map -- a rail onboarded at runtime shows up on the next scrape with no
  extra wiring.
- **Two naming tiers: label for shared, prefix for service-specific.** Metrics
  split into two groups, composed as two `flatten`ed sub-trees on `App`:
  - **Standard / cross-service** (`Standard`: `rpc`, `db`): the family names are the
    shared convention (`rpc_server_requests_total`, `db_client_duration_seconds`),
    so the producer is a constant `service="order-service"` **label**
    (`#[metrics(label(...))]`), letting them aggregate across the fleet.
  - **Service-specific** (`Business`: `orders`, `payments`, `jobs`): only this
    service defines them, so they live under its own `order_service_` **prefix**
    (`#[metrics(prefix = "order_service")]`) and carry *no* `service` label -- the
    prefix is the identity.

  Richer build identity stays in the top-level `service_info` metric.

Each module's top-of-file comment states the pattern and the reasoning, so
reading the source top to bottom is itself a guided tour.
