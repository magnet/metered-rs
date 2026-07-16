# Histograms in depth

Metered ships three recording engines for cumulative distributions:
`BucketHistogram`, `FixedExponentialHistogram`, and
`DynamicExponentialHistogram`. They share the wire model, the exemplar
machinery, and the query story. They differ in who chooses the buckets, what
memory costs, and what happens when your traffic surprises you. This section
gives the full comparison. For the one-paragraph version, read
[Choosing Metric Types](./metric-types.md).

## The classic bucket histogram

`BucketHistogram` records into boundaries you choose (`le` buckets), plus a
running `_sum` and `_count`.

**Benefits.** The boundaries carry meaning. Put a bucket edge exactly at your
latency objective, and the dashboard answers "how many requests beat the
objective" with no interpolation error. Memory cost and encode cost stay fixed
and small. Every scraper understands the output.

**Costs.** You must know the range before the first deploy. Boundaries that
miss the real distribution give useless resolution, and changing them later
breaks dashboard continuity. Each bucket is one series on the wire, so
resolution multiplies cardinality.

Use it when the boundaries are part of the contract: objective edges, fixed
size classes, or parity with an existing dashboard.

## Exponential histograms

The exponential engines remove the boundary decision. Bucket `i` covers
`[base^i, base^(i+1))` where `base = 2^(2^-schema)`. One integer, the
`schema`, sets the relative resolution everywhere at once:

| `schema` | Relative error per bucket |
| --- | --- |
| 0 | one power of two, about 100% |
| 3 | ~9% |
| 5 | ~2.2% |
| 8 | ~0.27% |

The error is *relative*, so one setting serves microseconds and minutes in
the same histogram. There is no boundary list to choose, tune, or migrate.
Non-positive values land in a dedicated zero bucket. The valid `schema` range
is 0 to 20.

### `FixedExponentialHistogram`: dense, bounded range

`FixedExponentialHistogram::try_new(min, max, schema)` allocates every bucket
between `min` and `max` up front.

**Benefits.** The observe path is fully lock-free with no branches for table
management. Memory is exact and known at construction. Construction fails
closed: an impossible range or schema is an error, not a surprise later.

**Costs.** You are back to declaring a range. Values outside it clamp to the
edge buckets. A wide range at a fine schema allocates many slots whether you
hit them or not.

Use it for one hot histogram whose range you genuinely know.

### `DynamicExponentialHistogram`: sparse, self-scaling

`DynamicExponentialHistogram::new()` starts at schema 5, about 2.2%
resolution, with a 256-slot table. `with_params(start_schema, capacity)` tunes both. This
is the engine the heavy-duty deployments run as their default, and the one
the rest of the stack assumes.

**Benefits.** Buckets live in a fixed-capacity lock-free table, and memory
tracks the buckets your values *populate*, not the range you configured. A
fleet of mostly idle histograms stays cheap. The observe path is one `log2`
and one atomic increment, with a one-time compare-and-swap when a value claims
a new bucket. Per-bucket exemplars are lock-free too.

**Costs.** The capacity is a budget. When the table saturates, the histogram
*downscales*: it merges adjacent buckets, which halves the resolution. The
counts already recorded coarsen with it. The schema only ever decreases.

A value range far wider than the capacity affords is not an error. The
histogram converges to a coarse but correct summary of the range.

**What saturation does not cost.** The rebuild never runs on the observe path.
An observation that finds the table full sets a flag. The merge happens on
the upkeep path, in `housekeep` (see
[the upkeep path](./core-model.md#the-upkeep-path-housekeep)).

Be precise about the lock-freedom claim: it covers the *observe* path only.
The upkeep pass is not lock-free. It allocates the replacement table and
takes a mutex over the retired-table list. That is fine, because it runs on
the scraping task, once per scrape, never on a recording thread. A
compare-and-swap gate keeps concurrent scrapers honest: one performs the
rebuild, the rest skip past it.

The swap uses read-copy-update: observers keep
using the old table lock-free until the histogram publishes the coarser one.
The drain then folds a straggler's late increment into the live table exactly
once. No observation is ever dropped or double-counted.

## Trade-offs at a glance

| | `BucketHistogram` | `FixedExponentialHistogram` | `DynamicExponentialHistogram` |
| --- | --- | --- | --- |
| You choose | every boundary | range + `schema` | `schema` + slot budget |
| Memory | fixed, per boundary | fixed, whole range | tracks populated buckets |
| Observe cost | lock-free branch scan | fully lock-free index | lock-free `log2` + add |
| Surprise range | clamps into edge buckets | clamps into edge buckets | downscales, keeps counting |
| Resolution over time | constant | constant | can coarsen, never below `schema` 0 |
| Failure mode | wrong boundaries forever | construction error | coarser buckets |

## Memory for common ranges

Per-bucket costs, from the struct layouts:

| Engine | Bytes per bucket | What a bucket holds |
| --- | --- | --- |
| `BucketHistogram` | ~24 B | boundary `f64` + count + exemplar slot |
| `FixedExponentialHistogram` | 8 B | count only; this engine has no per-bucket exemplar slots |
| `DynamicExponentialHistogram` | 32 B per table slot | index + count + exemplar slot + window state |

An exponential engine needs `log2(max / min) x 2^schema` buckets to span a
range. For ranges that services actually meter:

| Range | Spread | Buckets at `schema` 3 / 5 / 8 |
| --- | --- | --- |
| Cache operation: 1 µs to 10 ms | 10^4 | 107 / 426 / 3,402 |
| RPC latency: 100 µs to 10 s | 10^5 | 133 / 532 / 4,252 |
| Payload size: 64 B to 16 MiB | 2^18 | 144 / 576 / 4,608 |

What that costs per engine, on the RPC latency range:

- **Classic**, 14 hand-picked boundaries: ~360 B, and all 15 bucket series
  are on the wire at every scrape, populated or not.
- **Fixed** at `schema` 5: 532 buckets x 8 B = ~4.3 KiB, allocated up front
  whether traffic hits them or not. At `schema` 8 that becomes ~34 KiB.
  Only populated buckets reach the wire.
- **Dynamic** at the defaults: an 8 KiB table of 256 slots x 32 B, and that
  is the *ceiling for any range*. Only populated slots reach the wire.

The dynamic budget rule: `capacity / 2^schema` is how many powers of two
can populate before a downscale. The defaults give `256 / 32 = 8` powers of
two. That is a x256 spread at the full ~2.2% resolution. A healthy latency
distribution concentrates well inside that. A uniform flood across the full
x10^5 RPC range would settle at `schema` 3, ~133 populated buckets. That
still resolves ~9% per bucket, from the same 8 KiB.

The fleet math is where the engines separate. One thousand mostly idle
per-target histograms cost a fixed engine the full range each: ~4.3 MiB at
`schema` 5 on the RPC range. Dynamic tables only fill as targets actually
observe. The wire carries only what filled.

## Choosing

1. Boundaries are part of a contract, or a dashboard depends on exact edges:
   `BucketHistogram`.
2. One histogram, hot path, known range, and you want zero table management:
   `FixedExponentialHistogram`.
3. Everything else -- and especially many instances, unknown ranges, or
   per-target families: `DynamicExponentialHistogram`. This is the default
   worth reaching for first.

## Rendering and querying

Both exponential engines snapshot into the same `ExponentialSnapshot`, and
`metered-om` encodes a histogram family in either of two forms:

- **Classic `le` buckets.** Compatible with every Prometheus-style scraper
  and `histogram_quantile()`.
- **VictoriaMetrics `vmrange` series.** The native form for VictoriaMetrics;
  its query functions consume the ranges directly.

The family declares its encode intent and the sink resolves it against its
own capability, so the same metric definition serves both fleets. Bucket
exemplars attach identically in either encoding. The `quantile` values always
come from the query engine, at read time, so they aggregate correctly across
replicas -- the whole reason to prefer histograms over summaries.
