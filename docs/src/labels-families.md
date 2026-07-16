# Labels and families

Labels turn one metric into many series -- one per combination of label values.
They are the most useful and the most dangerous feature in metrics. This page
covers how to use them safely and the two ways Metered attaches them.

## Two kinds of labels

- **Constant labels** are the same on every series from a registry: `service`,
  `instance`, `region`, `env`. You set them once on the `Registry` /
  `MetricTreeView`, and every series carries them.
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
methods × 3 results = 60 series for one metric. That is fine. But a label with
unbounded values creates *unbounded* series and can overwhelm the backend.
Examples are a user id, an order id, a request id, a raw URL with query strings,
and a raw error message. This is "cardinality explosion."

The rule: **dimensional labels must stay bounded**, with values you mostly know
ahead of time.

Good dimensional labels: `route` from a fixed set of endpoints, `method`,
`result` with values `ok` / `error`, `upstream`, `mode`, and error *kind* as an
`enum`.

Bad: anything per-user, per-request, per-entity, or free-form. If you want one
of those, you usually want one of three things. Use a different metric. Use a
normalized category: status *class* `5xx` instead of the exact code, or route
*template* `/orders/{id}` instead of the concrete path. Or use an
[exemplar](./exemplars.md), which carries a trace id *without* a new series.

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

### Typed label keys, preferred

For a stable dimension, derive `LabelSet` on a key struct. Each field becomes a
label. The type makes the dimension explicit and prevents typos:

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
schema is correct before any traffic arrives. You can drop a stale series, such
as a closed connection or a removed route, with `Family::remove`.

## Keyed state as families

A family is a keyed subtree: one label set selects one member's metrics.
Metered gives that subtree two ownership modes. In the **owned** mode, a
`Family<L, M>` stores the members inside the family. In the **borrowed** mode,
a *family view* iterates members that your own state stores. The two modes
give the same wire output for the same logical data.

### Owned: whole metric structs per key

The member type `M` of a `Family<L, M>` is any `MetricTree`, not just a single
metric. A derived metric struct works as-is, so one key can own a whole bundle
of metrics:

```rust
use metered::{Counter, Family, Gauge, LabelSet, MetricTree};
use std::sync::atomic::{AtomicI64, AtomicU64};

#[derive(Clone, PartialEq, Eq, Hash, LabelSet)]
struct RailLabels {
    rail: String,
}

#[derive(Default, MetricTree)]
struct RailMetrics {
    #[metric(counter)]
    sent: AtomicU64,
    #[metric(gauge)]
    queue_depth: AtomicI64,
}

let rails: Family<RailLabels, RailMetrics> = Family::default();
rails.with(&RailLabels { rail: "sepa".to_owned() }, |m| {
    Counter::incr(&m.sent);
    Gauge::set(&m.queue_depth, 3);
});
```

The owned mode carries the machinery with it. A `MetricConstructor` builds
each member on first use. Metered sorts the series by label pairs. When key values
come from external input, bound them with `BoundedValues`: it interns values
up to a cap and maps the rest to one overflow value.

### Borrowed: family views over your own state

Often the keyed state already exists in your service: a map of rails, remotes,
or shards. The metrics live inside the members. Do not mirror that map into an
owned `Family`. Expose it borrowed instead, through
`MetricTreeView::family_view`, which keys members by a typed `LabelSet`:

```rust
use metered::entry::counter;
use metered::{LabelSet, MetricTreeView, MetricsView};
use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;

#[derive(Clone, PartialEq, Eq, Hash, LabelSet)]
struct RailLabels {
    rail: String,
    direction: String,
}

struct Rail {
    sent: AtomicU64,
}

impl MetricsView for Rail {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        let mut view = MetricTreeView::new();
        view.register(
            counter("sent")
                .select(|rail: &Rail| &rail.sent)
                .help("Payments sent on this rail"),
        );
        view
    }
}

struct Rails {
    map: RwLock<HashMap<RailLabels, Rail>>,
}

let mut view = MetricTreeView::with_prefix("rails");
view.family_view(Rail::metrics_view(), |rails: &Rails, out| {
    for (key, rail) in rails.map.read().unwrap().iter() {
        out.emit(key, rail);
    }
});
```

There is one borrowed-family primitive, `family_view`, and one layer of
sugar. `MetricTreeView::family_by` is `family_view` for the common
one-string-key case. It takes a label name and a closure that emits
`(key, member)` pairs. Internally, both forms feed the same per-member
emission seam. The string form stamps its one `(label, key)` pair borrowed
from the caller's `&str`, without a copy. And `Family` is the same contract
with Metered-owned storage: one keyed group of members rendered as one
labeled family, with the storage inverted.

The element shape comes from a context-free element view. Metered declares
the schema once, from `Rail::metrics_view()` alone. The schema declares the
key's label names without values. An empty group still advertises its
families. Membership churn appears automatically at the next scrape: your
map's inserts and removes are the lifecycle. For `family_view`, the key type
must declare its label names statically -- `#[derive(LabelSet)]` keys do,
`Vec<(String, String)>` does not.

Two disciplines transfer to you in the borrowed mode. Cardinality control
belongs to whatever admits entries into your map. The `iterate` closure runs
on the scrape path, so keep its lock scope small. Emission order is
caller-driven: the document lists members in the order you emit them. Sort in
`iterate` if you want the same sorted order as a `Family`.

Upkeep also forwards through the group. `housekeep` reaches every emitted
member, so dynamic histograms inside members keep rescaling.

### Which mode to use

| Question | Owned `Family<L, M>` | Borrowed family view |
|---|---|---|
| Storage owner | The family stores the members in its own map. | Your map or state stores the members. |
| Key type | A typed `LabelSet`, or `Vec<(String, String)>` with declared label names. | A typed `LabelSet` that declares its names with `family_view`, or one string label with `family_by`. |
| Cardinality control | Intern external-input keys with `BoundedValues`. | Whatever admits entries into your map. |
| Membership lifecycle | `Family::with` creates a member on first use; `Family::remove` drops one. | Your map's inserts and removes; the next scrape reflects them. |
| Lock discipline | `with` read-locks the family while your closure runs; keep it short. | `iterate` runs on the scrape path; keep its lock scope small. |
| Ordering | Sorted by label pairs. | Caller-driven; sort in `iterate` for parity. |

### One story on the wire

For the same logical data, an owned `Family<L, M>` and a borrowed
`family_view` write the same OpenMetrics document, byte for byte. The documents have
the same families, the same label names and values, and the same samples. A
test in the Metered repository asserts
that byte equality. Pick the mode by who owns the storage, not by the output.

## Where labels come from at exposition

A registered metric inherits the registry's constant labels. A `Family` adds its
dimensional labels on top. A `StateSet` adds a label named after the metric. An
`Info` carries its facts as labels. They compose, so the encoded series carry the
union -- for example `{service="orders",route="/orders",method="POST"}`.
