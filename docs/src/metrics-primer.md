# Metrics & OpenMetrics primer

This page is for readers who have not worked with Prometheus / OpenMetrics
before. If you already have, skim it for the vocabulary Metered uses and move on.

## What a metric is

A **metric** is a named, numeric measurement of your running program, sampled
over time. "Number of requests handled," "current queue depth," and "request
latency" are all metrics. A monitoring system **scrapes** your process
periodically, for example every 15 seconds. It reads the current numbers and
stores them as a time series it can graph and alert on.

OpenMetrics is the standard text format for that exchange, the successor to the
Prometheus exposition format. Metered produces it directly.

## Pull, and cumulative

Two ideas underpin everything else:

- **Pull, not push.** The monitoring system asks your process for its current
  numbers; your process does not send them anywhere. So a metric is just *state
  you can read on demand*, not an event you emit.
- **Cumulative, not reset.** A counter only ever goes up (for the life of the
  process). You never reset it after a scrape. The monitoring system stores each
  sample and computes differences itself. This is what lets two replicas be
  summed correctly, and what lets a scrape that arrives late or twice not corrupt
  your data.

Keep these in mind. They explain why Metered has no `flush` or `clear`
operation. They also explain why the query engine computes rates, not your
process.

## The metric types

OpenMetrics has a small set of types. Metered models each one.

### Counter

A value that only increases: requests handled, errors returned, bytes written.
On the wire, the encoder exposes a counter `http_requests` as
`http_requests_total`.

You do **not** expose a rate. You expose the running total. The query
`rate(http_requests_total[5m])` turns it into "requests per second." The query
computes it over whatever window the dashboard chooses, and it sums correctly
across instances.

### Gauge

A gauge goes up and down and represents *current state*. Examples are queue
depth, in-flight requests, connection-pool size, a temperature, and an on/off
flag (`1` or `0`). The monitoring system reads a gauge as-is at scrape time.

The litmus test: if the right thing to graph is the **value itself**, it is a
gauge. If the right thing to graph is **how fast it grew**, it is a counter.

### Histogram

A distribution, for things like latency. A histogram does not store every
observation. It counts how many fell into each of a fixed set of **buckets**
with upper bounds (`le`, "less than or equal"). It also keeps a running `sum`
and `count`. For
a latency metric `http_request_duration_seconds` you get series like:

```text
http_request_duration_seconds_bucket{le="0.005"} 24
http_request_duration_seconds_bucket{le="0.01"}  41
http_request_duration_seconds_bucket{le="+Inf"}  57
http_request_duration_seconds_sum               0.83
http_request_duration_seconds_count             57
```

Buckets are **cumulative** (`le="0.01"` includes everything `le="0.005"`
counted). The query engine computes percentiles at query time with
`histogram_quantile(0.95, ...)`. Because the buckets are plain counters, histograms from many replicas add
up, so a fleet-wide p95 is meaningful.

### Summary

An older shape that ships **pre-computed quantiles** (for example
`quantile="0.95"`) plus `sum`/`count`. The catch: you **cannot aggregate**
pre-computed quantiles --
you cannot average two replicas' p95s to get the fleet p95. Prefer histograms.
Metered offers summaries only for backwards compatibility (see
[Migrating](./migration.md)).

### Info

Static key/value facts about the process -- build version, commit, region --
exposed as a constant `1` that carries the facts as labels:

```text
build_info{version="0.10.0",commit="abc123"} 1
```

You join on it in queries to attach those facts to other metrics.

### StateSet

A set of mutually exclusive boolean states -- a lifecycle, say -- where exactly
one is `1` and the rest are `0`:

```text
service_lifecycle{service_lifecycle="starting"} 0
service_lifecycle{service_lifecycle="running"}  1
service_lifecycle{service_lifecycle="draining"} 0
```

## Labels

You can split a metric along **labels** -- key/value pairs that create one time
series per combination:

```text
http_requests_total{route="/orders",method="POST"} 12
http_requests_total{route="/orders",method="GET"}  87
```

Labels are powerful and dangerous. Every distinct combination of label values
is a **separate stored time series**. A label with unbounded values, such as a
user id, a request id, or a raw URL, creates unbounded series. This
"cardinality explosion" can take down your monitoring system. The rule: **keep
labels bounded.** [Labels and Families](./labels-families.md) covers this in
depth.

## Naming and units

Conventions that Metered follows and encourages:

- Use a `namespace_subsystem_name` shape: `http_request_duration_seconds`.
- Counters carry no rate in the name; the encoding adds the `_total` suffix. So
  name the field `requests`, not `requests_total` (or you get
  `requests_total_total`).
- **Put the unit in the name**, and use base units: **seconds**, not
  milliseconds, and **bytes**, not kilobytes. Use `_seconds` and `_bytes`.
  Metered records durations as seconds for this reason.

## The mental model to carry forward

A metric is **state you expose**, read at scrape time, cumulative where it
counts. Rates and percentiles are the monitoring system's job, not yours. The
next section shows how Metered turns that model into an API.
