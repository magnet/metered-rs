# Shaping names

A series' name comes from the path through the metric tree: the `Registry`
prefix, the registered name, then one segment per nesting level. A nesting level
is a struct field or a view entry. That makes the wire name a function of your
**code structure** -- so a refactor silently renames metrics:

- rename a struct field, and its series name changes;
- extract a few metrics into a sub-struct for tidiness, and they all gain a new
  segment.

Renamed metrics break dashboards and alerts. Name shaping decouples the wire name
from the code so you can refactor freely.

## Derive attributes

`#[derive(MetricTree)]` accepts two field attributes:

```rust
use metered::{Counter, Gauge, MetricTree};
use std::sync::atomic::{AtomicI64, AtomicU64};

#[derive(Default, MetricTree)]
struct PoolMetrics {
    #[metrics(counter)]
    acquired: AtomicU64,
    #[metrics(gauge)]
    idle: AtomicI64,
}

#[derive(Default, MetricTree)]
struct ApiMetrics {
    // The Rust field is `request_count`, but the wire segment stays `requests`.
    #[metrics(counter, rename = "requests")]
    request_count: AtomicU64,

    // Extracted into a sub-struct for organization, but flattened so the names
    // do not gain a `pool` segment: `api_acquired_total`, not
    // `api_pool_acquired_total`.
    #[metrics(flatten)]
    pool: PoolMetrics,
}
```

- `#[metrics(rename = "wire_name")]` sets the segment a field contributes.
- `#[metrics(flatten)]` drops the field's segment so its children sit at the
  parent level -- the metric-tree analogue of `#[serde(flatten)]`.

## A self-contained root: container `prefix` and `label`

A `#[derive(MetricTree)]` struct can carry its own name prefix and constant
labels, so a root metric tree needs no hand-wired `Registry`:

```rust
use metered::MetricTree;
use metered_om::OpenMetricsExt;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

#[derive(Default, MetricTree)]
#[metrics(prefix = "app", label(service = "orders", region = "eu"))]
struct AppMetrics {
    #[metrics(counter)]
    requests: AtomicU64,
    #[metrics(gauge)]
    queue_depth: AtomicI64,
}

let metrics = AppMetrics::default();
metrics.requests.fetch_add(1, Ordering::Relaxed);

// Rendered directly -- prefix and labels are baked in.
let text = metrics.encode_to_string().unwrap();
// app_requests_total{service="orders",region="eu"} 1
```

- `#[metrics(prefix = "...")]` joins a segment onto the inherited name.
- `#[metrics(label(key = "value", ...))]` appends constant labels to every family.

Both compose when you nest the tree under another, exactly like a `Registry`
prefix and `label`. `MetricTreeExt` provides `schema` / `values` to expose any
self-contained tree's schema and values. `metered_om::OpenMetricsExt` adds
`encode_to_string` to encode it without a `Registry`. Reach for a `Registry` or
`MetricTreeView` only when you compose several trees or apply the prefix and
labels at the exposition site instead.

## Programmatic adaptors

For hand-built trees and the `Registry`, the same control is available as the
`metered::shape` adaptors `Renamed` and `Flatten`:

```rust
use metered::entry::metric;
use metered::{Counter, Registry};
use metered::shape::{Flatten, Renamed};
use std::sync::atomic::AtomicU64;

# let hits = AtomicU64::new(0);
# let pool_tree = AtomicU64::new(0);
let renamed = Renamed::new("requests", &hits); // contributes the `requests` segment
let flattened = Flatten::new(&pool_tree);       // contributes no segment

let mut registry = Registry::new();
registry.register(metric("api").source(&renamed).help("API requests")); // emits `api_requests_total`
```

## The one invariant

Both the attributes and the adaptors apply the transform **uniformly** to
`describe` and `collect`, and therefore to the default `encode`. The schema and
the values can never disagree about a shaped name. There is no path where the
`# TYPE` line says one thing and the samples say another.

Shaping works on *segments* within the path, not on absolute names, so a
`Registry` prefix still composes correctly. A renamed or flattened subtree still
gets the prefix like everything else.
