# OpenMetrics exposition

The OpenMetrics text exposition lives in its own crate, **`metered-om`**.
The core `metered` crate only describes a `MetricSchema` and collects
`MetricValues` through the `MetricSink` trait. A service depends on a sink
crate and chooses it at the exposition site. Add both crates:

```toml
[dependencies]
metered = "0.10.0-rc.1"
metered-om = "0.10.0-rc.1"
```

`Registry` composes metric trees. The `OpenMetricsRegistryExt` trait adds the
`encode_to_string` convenience:

```rust
use metered::entry::counter;
use metered::{Counter, Registry};
use metered_om::OpenMetricsRegistryExt;
use std::sync::atomic::AtomicU64;

let requests = AtomicU64::new(0);
requests.incr();

let mut registry = Registry::with_prefix("demo");
registry.label("service", "api");
registry.register(counter("requests").source(&requests).help("Total requests handled"));

let text = registry.encode_to_string().unwrap();
assert!(text.contains("# HELP demo_requests Total requests handled"));
assert!(text.contains("demo_requests_total{service=\"api\"} 1"));
```

For reusable buffers or streaming responses, drive the encoder (a `MetricSink`)
over the schema/values directly:

```rust
use metered_om::OpenMetricsEncoder;

let schema = registry.schema();
let values = registry.values();

let mut text = String::new();
let mut encoder = OpenMetricsEncoder::new(&mut text);
encoder.encode_document(&schema, &values).unwrap();
encoder.finish().unwrap();
```

The encoder keys `HELP` and `UNIT` metadata to the exact metric family.
Metadata registered for a composite tree does not leak onto its child families.

## VictoriaMetrics `vmrange` buckets

The encoder receives each histogram whole, so the sink chooses the bucket
rendering. The default is the classic cumulative `le` form. Switch the encoder
to `vmrange` for VictoriaMetrics, an extension of the same OpenMetrics text
format. Exponential histograms emit `vmrange` natively, as non-cumulative
`lo...hi` ranges. Classic bucket histograms fall back to `le`:

```rust
use metered_om::{HistogramProfile, OpenMetricsEncoder};

let mut text = String::new();
let mut encoder =
    OpenMetricsEncoder::new(&mut text).histogram_profile(HistogramProfile::VmRange);
registry.encode(&mut encoder).unwrap();
encoder.finish().unwrap();
// latency_seconds_bucket{vmrange="5.000e-3...1.000e-2"} 3
```

## Rendering large metric sets incrementally

`encode_document` renders a whole document in one synchronous call. For very
large metric sets, `OpenMetricsRender` writes the same document in
budget-bounded steps so you can yield between chunks. An item is a family
declaration or a sample line. Each `step` emits at most `budget` items:

```rust
use metered_om::{OpenMetricsRender, RenderProgress};

let schema = registry.schema();
let values = registry.values();

let mut render = OpenMetricsRender::new(&schema, &values);
let mut text = String::new();
while render.step(&mut text, 256).unwrap() == RenderProgress::Pending {
    // hand control back to your loop/runtime between chunks
}
assert!(text.trim_end().ends_with("# EOF"));
```

The stepper is not `async`, so it works anywhere. Async callers can use
`RenderFuture`, a dependency-free `Future` that writes one budget chunk per poll
and yields back to the executor while more work remains:

```rust
# async fn scrape(schema: &metered::MetricSchema, values: &metered::MetricValues) {
use metered_om::RenderFuture;

let text = RenderFuture::new(schema, values, 256).await.unwrap();
# let _ = text;
# }
```

For tests and tooling, parse text exposition back into a structural model:

```rust
use metered_om::OpenMetricsDocument;

let doc = OpenMetricsDocument::parse(&text).unwrap();
let requests = doc.family("demo_requests").unwrap();
assert_eq!(requests.help.as_deref(), Some("Total requests handled"));
```

When metrics already live inside an app context, use `MetricTreeView<C>`.
It stores selector closures rather than metric references:

```rust
use metered::entry::counter;
use metered::MetricTreeView;
use metered_om::OpenMetricsViewExt;
use std::sync::atomic::AtomicU64;

struct App {
    requests: AtomicU64,
}

let app = App { requests: AtomicU64::new(0) };
let mut view = MetricTreeView::with_prefix("demo");
view.register(counter("requests").select(|app: &App| &app.requests).help("Total requests"));

let text = view.encode_to_string(&app).unwrap();
```

This keeps registry composition free of shared ownership: the app/context owns
the metrics, and the view only describes how to borrow them.

For existing non-metric state, register a direct reader:

```rust
# use metered::entry::gauge_value;
# use metered::MetricTreeView;
# struct App { enabled: bool }
# let app = App { enabled: true };
# let mut view = MetricTreeView::with_prefix("demo");
view.register(
    gauge_value("enabled")
        .read(|app: &App| app.enabled as i64)
        .help("Whether the app is enabled"),
);
```

## Adapting plain state with `Registry`

On the borrowed `Registry` path, the `adapter` metrics expose state that is not
itself a metric: an `AtomicBool`, a queue length, a running total. The adapter
reads the state at encode time, so the exported value is always live:

```rust
use std::sync::atomic::{AtomicBool, Ordering};
use metered::adapter::{flag, CounterFn};
use metered::entry::metric;
use metered::Registry;
use metered_om::OpenMetricsRegistryExt;

let enabled = AtomicBool::new(true);
let processed = std::sync::atomic::AtomicU64::new(7);

let flag_metric = flag(|| enabled.load(Ordering::Relaxed));     // gauge 0/1
let processed_metric = CounterFn(|| processed.load(Ordering::Relaxed));

let mut registry = Registry::new();
registry.register(metric("enabled").source(&flag_metric).help("Enabled"));
registry.register(metric("processed").source(&processed_metric).help("Processed"));
let text = registry.encode_to_string().unwrap();
```

When you own the type, prefer to implement `Metric` for it, so the type and its
value live in one place. The `adapter` metrics are for state you only want to
read.

## Serving foreign Prometheus text

Migrations are rarely all-or-nothing. Part of a process often still produces
classic Prometheus text: an older metrics stack, a sidecar, a library you do
not own. `TextSourceTree` keeps those metrics on the same scrape endpoint as
your native `metered` trees. It is a `MetricTree` that parses its source's
Prometheus text and re-emits the samples. It changes **no** name, label, or
value, so existing dashboards keep working unmodified while you migrate one
subsystem at a time.

The parser is lenient by design: it accepts the dialect that `serde_prometheus`
and similar producers emit. That dialect is looser than OpenMetrics: metadata
lines are optional, the parser tolerates spaces around `=` inside label braces,
and a counter can lack the `_total` suffix. Mount one `TextSourceTree` beside
your native trees:

```rust,no_run
use metered::entry::{counter, metric};
use metered::{Counter, Registry};
use metered_om::prom_text::TextSourceTree;
use metered_om::OpenMetricsRegistryExt;
use std::sync::atomic::AtomicU64;

// A native metered counter...
let requests = AtomicU64::new(0);
requests.incr();

// ...beside a foreign producer that already emits classic Prometheus text.
let legacy = TextSourceTree::new(|| {
    "legacy_hit_count{method=\"GetOrder\"} 42\n\
     legacy_response_seconds{method=\"GetOrder\",quantile=\"0.95\"} 0.250\n"
        .to_owned()
});

let mut registry = Registry::with_prefix("demo");
registry.register(counter("requests").source(&requests).help("Native requests"));
registry.register(metric("legacy").source(&legacy));

let text = registry.encode_to_string().unwrap();
// Native metrics carry the registry prefix...
assert!(text.contains("demo_requests_total 1"));
// ...while foreign samples are re-emitted exactly as parsed: no `demo_` prefix,
// no `_total` normalization, no invented `# TYPE` line.
assert!(text.contains("legacy_hit_count{method=\"GetOrder\"} 42"));
```

The tree calls the closure passed to `TextSourceTree::new` on every scrape, so
the exported values are always live. The tree emits each sample with the same
name, the same labels, and the same integral or float rendering as the source.
Only the label form changes: the output uses the canonical OpenMetrics form,
`k="v"` with no spaces. The bytes can differ, but the samples stay the same.
Foreign samples are deliberately untyped: classic Prometheus text carries no
`# TYPE` metadata, and an invented one would change the exposition. For that
reason the tree's mount name does not prefix foreign samples, and the encoder
writes no type line for them.

A scrape must never fail because a foreign source glitched. The parser drops
each line that it cannot parse. The good lines survive, and the endpoint still
returns `200`.

If the foreign source needs its own per-scrape maintenance, for example a swap
of an interval histogram, attach that work with `with_housekeep`. The tree's
`housekeep` drives the hook once per scrape cycle:

```rust,no_run
use metered_om::prom_text::TextSourceTree;

let legacy = TextSourceTree::new(|| produce_legacy_text())
    .with_housekeep(|| swap_interval_histograms());
# let _ = legacy;
# fn produce_legacy_text() -> String { String::new() }
# fn swap_interval_histograms() {}
```

Use `TextSourceTree` only during a migration. When a subsystem moves to native
`metered` families, drop the `TextSourceTree`. Expose the families directly so
they carry full schema metadata. If you only need the parsed samples, for a test
or a one-off transform, `parse_prometheus_text` returns them as `RawSample`s
without the `MetricTree` wrapper.

## Parsing exposition back

For tests and tooling, parse OpenMetrics text into a structural model rather than
matching strings:

```rust
# let text = "# TYPE demo_requests counter\ndemo_requests_total 1\n# EOF\n";
use metered_om::OpenMetricsDocument;

let doc = OpenMetricsDocument::parse(text).unwrap();
assert_eq!(doc.families.len(), 1);
assert_eq!(doc.sample("demo_requests_total").map(|s| s.value.as_str()), Some("1"));
```
