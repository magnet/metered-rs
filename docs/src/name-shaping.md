# Shaping Names

A series' name is built from the path through the metric tree: the `Registry`
prefix, the registered name, then one segment per nesting level (a struct field,
a `#[metered]` method, a view entry). That makes the wire name a function of your
**code structure** -- which means refactoring silently renames metrics:

- rename a struct field, and its series is renamed;
- extract a few metrics into a sub-struct for tidiness, and they all gain a new
  segment;
- rename a measured method, and its metrics move.

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
    #[metrics(tree)]
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

## On `#[metered]` methods

This section describes the method-instrumentation API. Add the `metered-semantic`
crate to use `#[metered]`, `#[measure]`, and the semantic wrappers.

The same `#[metric(rename = "...")]` works on a measured method. The generated
registry field keeps the method name (so the Rust API is unchanged); only the
emitted segment changes:

```rust
use metered_semantic::{metered, HitCount};

struct Worker;

#[metered(registry = WorkerMetrics)]
impl Worker {
    #[measure(HitCount)]
    #[metric(rename = "run")] // segment stays `run` even if the method is renamed
    fn run_iteration(&self) {}
}
```

(Only `rename` applies to a method; `flatten` does not, since a method's metrics
are a sub-registry.)

## A self-contained root: container `prefix` and `label`

A `#[derive(MetricTree)]` struct can carry its own name prefix and constant
labels, so a root metric tree needs no hand-wired `Registry`:

```rust
use metered::{Counter, Gauge, MetricTree};
use metered_om::OpenMetricsExt;
use std::sync::atomic::{AtomicI64, AtomicU64};

#[derive(Default, MetricTree)]
#[metrics(prefix = "app", label(service = "orders", region = "eu"))]
struct AppMetrics {
    #[metrics(counter)]
    requests: AtomicU64,
    #[metrics(tree)]
    queue_depth: AtomicI64,
}

let metrics = AppMetrics::default();
Counter::incr(&metrics.requests);

// Rendered directly -- prefix and labels are baked in.
let text = metrics.encode_to_string().unwrap();
// app_requests_total{service="orders",region="eu"} 1
```

- `#[metrics(prefix = "...")]` joins a segment onto the inherited name.
- `#[metrics(label(key = "value", ...))]` appends constant labels to every family.

Both compose when the tree is nested under another, exactly like a `Registry`
prefix and `label`. `MetricTreeExt` (`schema` / `values`) exposes any
self-contained tree's schema and values; `metered_om::OpenMetricsExt`
adds `encode_to_string` to render it without a `Registry` -- reach for a
`Registry` or `MetricTreeView` only when composing several trees or applying the
prefix/labels at the exposition site instead.

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

## The one guarantee

Both the attributes and the adaptors apply the transform **uniformly** to
`describe` and `collect` (and therefore to the default `encode`). The schema and
the values can never disagree about a shaped name -- there is no path where the
`# TYPE` line says one thing and the samples say another.

Because shaping works on *segments* within the path (not absolute names), a
`Registry` prefix still composes correctly: a renamed/flattened subtree is still
prefixed like everything else.
