# Explicit Recording

Use `metered_semantic::recording` when code needs simple operation measurement
without a `tracing` span. Enable the `metered-semantic` `recording` feature:

```toml
[dependencies]
metered-semantic = { version = "0.10", features = ["recording"] }
```

Prefer `metered-tracing` for service RPC/HTTP operation metrics. Use recording
for local/internal operations where explicit metric state is clearer than a span
or where tracing is not available.

```rust
use metered_semantic::recording::Operation;

let refresh = Operation::default();

let result = refresh.record(|| Ok::<_, &'static str>(()));
assert!(result.is_ok());
```

`Operation` records `started`, `completed`, `failed`, `in_flight`, and
`duration_seconds` metric families. It implements `MetricTree`, so register it
with `entry::metric("refresh").source(&refresh)` or select it from a
`MetricTreeView`.

`Operation` treats `Err` results and aborted executions as failed. It does not
export Rust enum variants as metric labels or decide error categories for your
service. Use traces and logs for detailed error diagnosis, and bounded error
classes at service boundaries.
