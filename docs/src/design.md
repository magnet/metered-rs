# Why Metered Is Designed This Way

Metered makes a handful of strong, opinionated choices. Each one follows from the
mental model in the [primer](./metrics-primer.md): a metric is *state you expose*,
pulled and cumulative. This chapter explains the choices, because understanding
them makes the API feel obvious.

## Architecture at a glance

Three independent concerns meet at the metric state your service owns:

1. **Metric state** is readable service state: counters, gauges, histograms,
   info, state sets, and families.
2. **Composition** describes how to borrow that state at scrape time through
   `MetricTree`, `Registry`, and `MetricTreeView`.
3. **Instrumentation** updates state from tracing spans, explicit recording, or
   legacy method wrappers.

Core `metered` owns the first two concerns. It does not decide where service
operations begin or how errors should be categorized.

```mermaid
flowchart TD
    subgraph svc["Your service (owns all metric state)"]
        code["explicit operation code"]
        spans["tracing spans"]
        state["metric state<br/>Counter/Gauge implementors<br/>Histogram / Family / Info / StateSet"]
        code -->|"recording feature or legacy wrappers:<br/>enter → Recorder → record once"| state
        spans -->|"metered-tracing:<br/>span metrics + exemplars"| state
    end

    subgraph expo["Exposition (at scrape time)"]
        tree["MetricTree<br/>composed by Registry / MetricTreeView"]
        schema["MetricSchema<br/>(the shape)"]
        values["MetricValues<br/>(the samples)"]
        render["MetricSink<br/>e.g. metered-om:<br/>OpenMetricsEncoder / OpenMetricsRender"]
        tree -->|describe| schema
        tree -->|collect| values
        schema --> render
        values --> render
    end

    state -.->|"borrowed at scrape, no Arc"| tree
    render --> text["OpenMetrics text"]
    text --> scraper["Prometheus / VictoriaMetrics"]
    schema -.->|dashboard_queries| promql["PromQL dashboard seeds"]
```

The rest of this chapter is *why* each of those pieces looks the way it does.

## Metrics are state, not a shadow system

The central idea: **a metric is a piece of your service's state**, owned by the
component whose behavior it describes. A queue's depth metric *is* the queue's
length. A pool's "in use" gauge *is* the pool's checked-out count.

So Metered has **no global registry and no statics**. You do not "register a
metric with the metrics system" and then find it again by string name. You hold
concrete metric state (for example an `AtomicU64` counter, a histogram, or a
generated legacy registry) as a field, exactly where the relevant state lives,
and expose it when scraped. This means:

- No name-based lookups, no typos resolved at runtime, no init ordering.
- No accidental sharing: two subsystems cannot clobber each other's metric by
  using the same global name.
- The borrow checker keeps instrumentation honest -- a metric cannot outlive the
  thing it measures.

A consequence you will feel: **prefer exposing existing state over maintaining a
parallel counter.** If the queue knows its length, read the queue; do not
increment a separate gauge on every push and pop and hope it never drifts.

## No `Arc`, even across `.await`

Instrumentation must not force you to wrap your service in `Arc`, and must work
in `async` code where the body holds `&mut self` across an `.await`.

Metered's explicit recording and legacy models are two-phase (see
[Core Model](./core-model.md)): `Measure::enter` returns an owned **recorder**
that holds whatever lightweight handle it needs (an `Arc` to an atomic,
internally and `#[doc(hidden)]`), and the metric itself is borrowed only briefly
to enter and again to record -- never across the measured body. That is what lets
a measured `async fn` take `&mut self` and `.await` freely while still recording
exactly once.

For exposition, the same principle drives `MetricTreeView`: it stores *selector
closures*, not metric references, and borrows the live service at scrape time. No
`Arc` on every metric, no shared ownership just to render.

## Record exactly once -- even on panic or cancellation

A recorder is "armed" on creation and fires its observation exactly once:
`complete` on a normal return, or its `Drop` on a panic, an early `return`, or an
async task being cancelled. So `recording::Operation` and the legacy
method-level wrappers do not leak in-flight state or lose duration observations
when the body exits abnormally.

## OpenMetrics-native, no `serde` on the default path

Metered writes the OpenMetrics text format directly. It does not serialize
metrics to a generic data model and then map field names to Prometheus
conventions.

Why it matters:

- **Fidelity.** Counter `_total` suffixes, histogram `_bucket`/`_sum`/`_count`,
  `# TYPE`/`# HELP`/`# UNIT` metadata, exemplars, and stateset semantics are
  first-class, not approximated by reshaping JSON.
- **Dependency hygiene.** The default public API has no foreign types, and the
  `legacy` feature adds none, so a `metered` upgrade never forces a `serde` or
  `hdrhistogram` bump on your workspace. (See
  [Feature Flags and Stability](./features.md).)

## Cumulative histograms over in-process summaries

Older metrics libraries (and pre-computed HDR summaries) compute
quantiles in your process and expose them as a summary. The problem is
aggregation: you cannot combine two replicas' pre-computed p95s into a fleet p95.

Metered's operation duration paths (`metered-tracing`, `recording::Operation`,
and legacy `Elapsed`) record into cumulative bucket **histograms**. Percentiles
are computed at query time with `histogram_quantile`, where they aggregate across
replicas correctly. The bucket counters are also lock-free, so the hot path never
blocks.

## A lock-free, allocation-free hot path

Stock metrics back their state with atomics allocated once at construction. The
recording path -- `inc`, `observe`, gauge `set` -- is a relaxed atomic operation
with no allocation and no mutex. (The one exception, per-bucket exemplars, is
guarded by a mutex touched only when an exemplar is actually supplied, off the
counting path.) With the `legacy` feature, the guidance follows: use cheap
metrics like `HitCount` in the hottest paths, and reserve richer ones like
`Elapsed` for entry points.

## Schema, values, and rendering are separate

Every metric tree can do two independent things: **describe** its schema (family
names, types, units, label names) and **collect** its current values. The
renderer combines a schema and a value set into OpenMetrics text.

This split buys a lot:

- The schema can drive documentation and dashboard generation without a live
  scrape ([Schema and Dashboards](./schema-dashboards.md)).
- Values can be rendered incrementally, with a budget, for very large metric
  sets ([OpenMetrics Exposition](./openmetrics.md)).
- Because a single `MetricTree::encode` is *defined* as "describe + collect +
  render", a tree can never advertise one shape in its schema and emit another in
  its samples. The consistency is structural, not a convention.

## A type for the leaf, a trait for the tree

Two traits, with one job each:

- **`Metric`** is implemented by a single OpenMetrics family (a `Counter`
  implementor, a `Gauge` implementor, a histogram). It couples the family's
  *type* and its *value collection* in one place, so they cannot drift.
- **`MetricTree`** is implemented by anything composed of families: a legacy
  `#[metered]` registry, a `#[derive(MetricTree)]` struct, a `Family`. Leaves get it for free
  via a blanket implementation.

You implement `Metric` for a new leaf, `MetricTree` (usually by deriving) for a
composite. That is the whole extension story.

## Evolvable on purpose

Metered expects to reach 1.0 without churning its callers:

- Open enums like `MetricType` are `#[non_exhaustive]`, so new OpenMetrics
  constructs can be added without breaking `match`es.
- The macros emit `::metered::` absolute paths, so generated code is immune to
  local name shadowing.
- The procedural and derive macros are re-exported from `metered`, so downstream
  crates depend on `metered` alone and the two halves always move together.

With the "why" in hand, the [Core Model](./core-model.md) introduces the concrete
types.
