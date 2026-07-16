# Labels and Families

Labels turn one metric into many series -- one per combination of label values.
They are the most useful and the most dangerous feature in metrics. This chapter
covers using them safely and the two ways Metered attaches them.

## Two kinds of labels

- **Constant labels** are the same on every series from a registry: `service`,
  `instance`, `region`, `env`. You set them once on the `Registry` /
  `MetricTreeView`, and they are attached to everything.
- **Dimensional labels** vary per observation: `route`, `method`, `result`,
  `upstream`. Each distinct value is a separate time series. These are the ones
  that need discipline.

```rust
use metered::Registry;

let mut registry = Registry::with_prefix("orders");
registry.label("service", "orders");   // constant: on every series
registry.label("instance", "i-1");
```

## Cardinality: the one rule

Every distinct combination of dimensional label values is a **separate stored
time series** in your monitoring system. The cost is multiplicative: 5 routes × 4
methods × 3 results = 60 series for one metric. That is fine. But a label whose
values are unbounded -- a user id, an order id, a request id, a raw URL with query
strings, a raw error message -- creates *unbounded* series and can overwhelm the
backend. This is "cardinality explosion".

The rule: **dimensional labels must be bounded** and known-ish ahead of time.

Good dimensional labels: `route` (a fixed set of endpoints), `method`, `result`
(`ok` / `error`), `upstream`, `mode`, error *kind* (an enum).

Bad: anything per-user, per-request, per-entity, or free-form. If you find
yourself wanting one of those, you usually want: a different metric, a normalized
category (status *class* `5xx`, not the exact code; route *template*
`/orders/{id}`, not the concrete path), or an [exemplar](./exemplars.md) (which
carries a trace id *without* creating a series).

## Adding a dimension with `Family`

A `Family<L, M>` keeps one metric `M` per label set `L`, creating them on first
use -- the analogue of a Prometheus metric family.

### Dynamic label sets

For ad-hoc string labels, declare the label *names* up front so the schema does
not depend on which values traffic happens to produce:

```rust
use metered::{Counter, Family};
use std::sync::atomic::AtomicU64;

let by_route: Family<Vec<(String, String)>, AtomicU64> =
    Family::with_label_names(["route"]);

by_route.with(&vec![("route".to_owned(), "/health".to_owned())], |c| c.incr());
```

### Typed label keys (preferred)

For a stable dimension, derive `LabelSet` on a key struct. Each field becomes a
label; the type makes the dimension explicit and prevents typos:

```rust
use metered::{Counter, Family, LabelSet};
use std::sync::atomic::AtomicU64;

#[derive(Clone, PartialEq, Eq, Hash, LabelSet)]
struct RouteKey {
    route: String,
    method: String,
}

let requests: Family<RouteKey, AtomicU64> = Family::default();
requests.with(
    &RouteKey { route: "/orders".into(), method: "POST".into() },
    |c| c.incr(),
);
```

`Family::default()` reads the label names from the derived `LabelSet`, so its
schema is correct before any traffic arrives. You can drop a series that has gone
stale (a closed connection, a removed route) with `Family::remove`.

## Where labels come from at exposition

A registered metric inherits the registry's constant labels; a `Family` adds its
dimensional labels on top; a `StateSet` adds a label named after the metric; an
`Info` carries its facts as labels. They compose, so the encoded series carry the
union -- for example `{service="orders",route="/orders",method="POST"}`.
