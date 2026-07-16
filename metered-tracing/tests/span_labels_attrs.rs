//! Container-attribute escape hatches on `#[derive(SpanLabels)]`:
//! `macro_name = "..."` (crate-root opener collisions) and `crate = "..."`
//! (the emitted runtime path, for facade-only consumers).

use metered::{DynamicExponentialHistogram, Family, Registry, entry::metric};
use metered_om::OpenMetricsRegistryExt;
use metered_tracing::{SpanDurations, TracingMetrics, metered_info_span};
use std::sync::Arc;
use tracing_subscriber::prelude::*;

/// Two same-named labels types in different modules: the derive
/// `#[macro_export]`s each call-site opener at the crate root under the
/// type's name, so without an override the second type would collide
/// ("the name `QueryLabels` is defined multiple times"). This module pair is
/// the compile fixture: `redis::QueryLabels` renames its opener via
/// `#[span(macro_name = "redis_query_labels")]`, and both coexist.
mod sql {
    #[derive(Clone, PartialEq, Eq, Hash, metered::LabelSet, metered_tracing::SpanLabels)]
    #[span(name = "db.sql.query")]
    pub struct QueryLabels {
        #[span("db.operation.name")]
        pub db_operation: String,
    }
}

mod redis {
    #[derive(Clone, PartialEq, Eq, Hash, metered::LabelSet, metered_tracing::SpanLabels)]
    #[span(name = "db.redis.query", macro_name = "redis_query_labels")]
    pub struct QueryLabels {
        #[span("db.operation.name")]
        pub db_operation: String,
    }
}

fn sample_value<'a>(text: &'a str, series: &str) -> Option<&'a str> {
    text.lines()
        .find(|line| line.starts_with(series))
        .map(|line| line.rsplit_once(' ').map(|(_, v)| v).unwrap_or(line))
}

#[test]
fn same_named_types_in_different_modules_coexist_with_macro_name_override() {
    struct Db {
        sql: Family<sql::QueryLabels, DynamicExponentialHistogram>,
        redis: Family<redis::QueryLabels, DynamicExponentialHistogram>,
    }
    let db = Arc::new(Db {
        sql: Family::default(),
        redis: Family::default(),
    });
    let layer = TracingMetrics::builder()
        .recorder(SpanDurations::on(sql::QueryLabels::SPAN, &db, |db: &Db| {
            &db.sql
        }))
        .recorder(SpanDurations::on(
            redis::QueryLabels::SPAN,
            &db,
            |db: &Db| &db.redis,
        ))
        .build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        // The default opener still carries the type's name...
        metered_info_span!(QueryLabels; db_operation = "select".to_owned()).in_scope(|| {});
        // ...and the renamed one dispatches by its `macro_name`.
        metered_info_span!(redis_query_labels; db_operation = "get".to_owned()).in_scope(|| {});
    });

    let mut registry = Registry::new();
    registry.register(metric("sql_duration_seconds").source(&db.sql));
    registry.register(metric("redis_duration_seconds").source(&db.redis));
    let text = registry.encode_to_string().expect("render durations");
    assert_eq!(
        sample_value(&text, "sql_duration_seconds_count{db_operation=\"select\"}"),
        Some("1"),
        "{text}"
    );
    assert_eq!(
        sample_value(&text, "redis_duration_seconds_count{db_operation=\"get\"}"),
        Some("1"),
        "{text}"
    );
}

/// Simulated facade for the `#[span(crate = "...")]` override: the generated
/// code reaches the runtime through this re-export module instead of
/// `::metered_tracing` directly. A true `::metered::tracing` fixture would
/// need a dev-dependency cycle on the facade crate, so this stands in for it;
/// the derive only splices the path tokens, so any path with the crate's
/// items at its root exercises the same seam.
mod facade {
    pub use metered_tracing::*;
}

#[derive(Clone, PartialEq, Eq, Hash, metered::LabelSet, metered_tracing::SpanLabels)]
#[span(name = "facade.op", crate = "crate::facade")]
struct FacadeLabels {
    #[span("facade.kind")]
    kind: String,
    // An `on_close` field compile-exercises the generated `record_*` fn
    // (which also reaches `tracing` through the overridden path).
    #[span("facade.status", default = "unset", on_close)]
    status: String,
}

#[test]
fn crate_override_routes_generated_code_through_the_given_path() {
    struct Ops {
        duration: Family<FacadeLabels, DynamicExponentialHistogram>,
    }
    let ops = Arc::new(Ops {
        duration: Family::default(),
    });
    let layer = TracingMetrics::builder()
        .recorder(SpanDurations::on(FacadeLabels::SPAN, &ops, |ops: &Ops| {
            &ops.duration
        }))
        .build();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        let span = metered_info_span!(FacadeLabels; kind = "read".to_owned());
        FacadeLabels::record_status(&span, "ok".to_owned());
        span.in_scope(|| {});
    });

    let mut registry = Registry::new();
    registry.register(metric("facade_op_duration_seconds").source(&ops.duration));
    let text = registry.encode_to_string().expect("render duration");
    assert_eq!(
        sample_value(
            &text,
            "facade_op_duration_seconds_count{kind=\"read\",status=\"ok\"}"
        ),
        Some("1"),
        "{text}"
    );
}
