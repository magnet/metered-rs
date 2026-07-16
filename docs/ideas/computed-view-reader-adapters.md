# Idea (deferred): typed reader adapters for `ComputedView`

Status: **parked.** We decided to instead make sources (e.g. a service's
`RegistrationMetrics`) *directly composable* by implementing `metered::MetricTree`
in the source crate, so a consumer composes them with no adapter at all. This doc
keeps the typed-adapter design around in case we want it for sources we don't own.

## Problem

Exposing a foreign/computed source's metrics (values read live via getters)
without either:

- a boxed reader closure behind an `Arc` (the old `bridge::BridgedCounter`), or
- a stringly-typed attribute macro (`#[view(get = "method")]` — "black magic").

The wanted shape: define the view as a **struct of typed adapter fields** + a
proc macro, with **no `Arc` in the view type**.

## Adapters (shared by both options)

A reader is just a function pointer — no `Arc`, `Copy`, zero-cost:

```rust
// metered
pub struct CounterReader<C>(pub fn(&C) -> u64);
pub struct GaugeReader<C>(pub fn(&C) -> i64);
```

How the context instance is passed (identical in A and B): the parent owns the
single `Arc<C>` and its generated `MetricTree::describe`/`collect` dereferences it
and hands `&C` to the view's `collect_prefixed`. The reader closures call
`reader(&that)`.

## Option A — static view, readers in `Default`/`const`

```rust
pub trait ComputedView<C> {
    fn computed_view() -> MetricTreeView<'static, C>;   // static
}

#[derive(MetricsView)]
#[metrics(over = RegistrationMetrics, prefix = "registration_cleanup")]
struct RegistrationCleanup {
    #[view(help = "cleanup debt entries dropped")]
    debt_dropped: CounterReader<RegistrationMetrics>,
    #[view(help = "outstanding cleanup debt")]
    debt_gauge:   GaugeReader<RegistrationMetrics>,
}

// readers live here (A's wart: split from the field decl)
impl Default for RegistrationCleanup {
    fn default() -> Self {
        Self {
            debt_dropped: CounterReader(|m| m.debt_dropped_count()),
            debt_gauge:   GaugeReader(|m| m.debt_gauge() as i64),
        }
    }
}
```

Generated:

```rust
impl ComputedView<RegistrationMetrics> for RegistrationCleanup {
    fn computed_view() -> MetricTreeView<'static, RegistrationMetrics> {
        let this = <Self as Default>::default();
        let mut view = MetricTreeView::with_prefix("registration_cleanup");
        view.counter_value("debt_dropped", this.debt_dropped.0).help("...");
        view.gauge_value("debt_gauge", this.debt_gauge.0).help("...");
        view
    }
}
```

Parent (one field) + mount:

```rust
#[derive(MetricTree)]
struct RegistrationMetricsState {
    #[metric(flatten)] attempts: RegistrationAttemptMetrics,
    #[metric(view = "RegistrationCleanup")] cleanup: Arc<RegistrationMetrics>,
}
// generated collect: <RegistrationCleanup as ComputedView<_>>::computed_view()
//                        .collect_prefixed(&*self.cleanup, Some(name), labels, values);
```

## Option B — instance view, readers inline

```rust
pub trait ComputedView<C> {
    fn computed_view(&self) -> MetricTreeView<'static, C>;   // &self
}
```

Same view struct; readers written inline at construction (no `Default`):

```rust
impl ComputedView<RegistrationMetrics> for RegistrationCleanup {
    fn computed_view(&self) -> MetricTreeView<'static, RegistrationMetrics> {
        let mut view = MetricTreeView::with_prefix("registration_cleanup");
        view.counter_value("debt_dropped", self.debt_dropped.0).help("...");
        view.gauge_value("debt_gauge", self.debt_gauge.0).help("...");
        view
    }
}
```

Parent holds **two** fields (view value + context), wired by the mount:

```rust
#[derive(MetricTree)]
struct RegistrationMetricsState {
    #[metric(flatten)] attempts: RegistrationAttemptMetrics,
    #[metric(view_over = "cleanup")] cleanup_view: RegistrationCleanup,
    #[metric(skip)] cleanup: Arc<RegistrationMetrics>,
}
// generated collect: self.cleanup_view.computed_view()
//                        .collect_prefixed(&*self.cleanup, Some(name), labels, values);
```

## Comparison

| | A (static) | B (instance) |
|---|---|---|
| `computed_view` | static | `&self` |
| readers defined in | `Default`/`const` (split from decl) | inline at construction (one place) |
| parent fields for cleanup | 1 | 2 |
| mount | `#[metric(view = "...")]` | `#[metric(view_over = "...")]` |
| runtime-varying readers | no | yes |
| wart | reader decl split | extra parent field + 2-field mount |

Leaning A if revived. But for sources we *own*, the chosen approach (impl
`MetricTree` directly in the source crate) beats both — no adapter at all.
