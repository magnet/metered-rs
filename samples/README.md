# Metered Samples

These samples show individual APIs and edge cases. They are not the recommended
service architecture. For the clean service-level pattern, see
`examples/order-service`.

Every sample is compiled by CI: `samples/` is a workspace member whose library
includes each file as a module, so `cargo build --workspace` fails the moment a
sample drifts from the real API.

- `counter_gauge.rs`: disambiguating counters and gauges backed by `AtomicU64`.
- `family_labels.rs`: typed labels with `Family`.
- `metric_tree_derive.rs`: structural `MetricTree` derive, including per-field
  `gauge`/`counter`/`help`/`unit`.
- `metric_view_context.rs`: wiring metrics through an app context with typed
  entry builders (`entry::counter(...).select(...)`) on a `MetricTreeView`.
- `recording_operation.rs`: explicit non-tracing operation measurement.
- `tracing_span_metrics.rs`: a semantic span-derived metric family (`SpanMetric`).
- `grpc_semconv_profile.rs`: semconv-shaped RPC metrics derived from `rpc.server`
  spans, method in a label.
- `exponential_histogram.rs`: fixed/dynamic exponential histograms and `vmrange`.
- `legacy_method_macros.rs`: compatibility-only old method macros.
