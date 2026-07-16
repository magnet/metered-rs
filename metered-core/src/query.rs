//! Query-expression generation from a [`MetricSchema`](crate::schema), for both
//! Prometheus PromQL and VictoriaMetrics MetricsQL.
//!
//! Given a schema, produce high-signal query building blocks (counter rates,
//! gauge/state panels, histogram percentiles, and a heatmap seed) for dashboard
//! panels. The histogram queries differ by [`QueryDialect`]: PromQL groups
//! buckets by `le`; MetricsQL groups by `vmrange` and wraps heatmaps in
//! `prometheus_buckets`. This depends only on the schema; the schema knows
//! nothing about queries.

use crate::metric_tree::MetricType;
use crate::schema::MetricSchema;

/// The query dialect to generate.
///
/// `#[non_exhaustive]`: further dialects may be added without a breaking change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum QueryDialect {
    /// Prometheus PromQL: histogram buckets are grouped by `le`.
    PromQl,
    /// VictoriaMetrics MetricsQL: histogram buckets are grouped by `vmrange`,
    /// and heatmaps use `prometheus_buckets` to convert them to `le`.
    MetricsQl,
}

impl QueryDialect {
    /// The histogram bucket label this dialect groups by.
    fn bucket_label(self) -> &'static str {
        match self {
            QueryDialect::PromQl => "le",
            QueryDialect::MetricsQl => "vmrange",
        }
    }
}

/// A generated query building block suitable for a dashboard panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// Human-friendly panel title.
    pub title: String,
    /// The query expression, written in [`Query::dialect`].
    pub expr: String,
    /// What question this query answers.
    pub purpose: String,
    /// The dialect `expr` is written in.
    pub dialect: QueryDialect,
}

impl MetricSchema {
    /// Generates high-signal queries from the schema, in the given dialect.
    ///
    /// A strong starting point -- counter rates, gauge/state panels, histogram
    /// p50/p95/p99 and a heatmap seed with the right bucket grouping -- not a
    /// replacement for a dashboard author.
    pub fn queries(&self, dialect: QueryDialect) -> Vec<Query> {
        let mut queries = Vec::new();
        for family in self.families() {
            match family.metric_type {
                MetricType::Counter => queries.push(Query {
                    title: format!("{} rate", family.name),
                    expr: rate_query(&family.name, &family.labels),
                    purpose: "Throughput over time".to_owned(),
                    dialect,
                }),
                MetricType::Gauge
                | MetricType::StateSet
                | MetricType::Info
                | MetricType::Unknown => queries.push(Query {
                    title: family.name.clone(),
                    expr: instant_query(&family.name, &family.labels),
                    purpose: "Current state".to_owned(),
                    dialect,
                }),
                MetricType::Histogram => {
                    queries.extend(quantile_panels(
                        &family.name,
                        "Latency percentile from histogram buckets",
                        dialect,
                        |quantile| {
                            histogram_quantile_query(
                                &family.name,
                                &family.labels,
                                quantile,
                                dialect,
                            )
                        },
                    ));
                    queries.push(Query {
                        title: format!("{} heatmap", family.name),
                        expr: heatmap_query(&family.name, &family.labels, dialect),
                        purpose: "Bucket distribution over time (Grafana heatmap)".to_owned(),
                        dialect,
                    });
                }
                MetricType::Summary => queries.push(Query {
                    title: format!("{} quantiles", family.name),
                    expr: summary_quantiles_query(&family.name, &family.labels),
                    purpose: "Reported summary quantiles".to_owned(),
                    dialect,
                }),
                MetricType::GaugeHistogram => {
                    queries.extend(quantile_panels(
                        &family.name,
                        "Quantile from current gauge-histogram buckets",
                        dialect,
                        |quantile| {
                            format!(
                                "histogram_quantile({quantile}, {})",
                                gauge_buckets_by(&family.name, &family.labels)
                            )
                        },
                    ));
                    queries.push(Query {
                        title: format!("{} distribution", family.name),
                        expr: gauge_buckets_by(&family.name, &family.labels),
                        purpose: "Current bucket distribution (gauge histogram)".to_owned(),
                        dialect,
                    });
                }
            }
        }
        queries
    }
}

/// The shared p50/p95/p99 quantile-panel set used by histogram-shaped metrics
/// (`Histogram` and `GaugeHistogram`), which compute quantiles from buckets via
/// `histogram_quantile`. `expr_for` receives each quantile string (e.g. `"0.50"`)
/// and yields that panel's query expression; the title and purpose are uniform.
fn quantile_panels(
    name: &str,
    purpose: &str,
    dialect: QueryDialect,
    mut expr_for: impl FnMut(&str) -> String,
) -> Vec<Query> {
    ["0.50", "0.95", "0.99"]
        .iter()
        .copied()
        .map(|quantile| Query {
            title: format!("{name} p{}", quantile.trim_start_matches("0.")),
            expr: expr_for(quantile),
            purpose: purpose.to_owned(),
            dialect,
        })
        .collect()
}

fn rate_query(name: &str, labels: &[String]) -> String {
    let series = format!("{name}_total");
    if labels.is_empty() {
        format!("sum(rate({series}[$__rate_interval]))")
    } else {
        format!(
            "sum by ({}) (rate({series}[$__rate_interval]))",
            labels.join(", ")
        )
    }
}

fn instant_query(name: &str, labels: &[String]) -> String {
    if labels.is_empty() {
        name.to_owned()
    } else {
        format!("sum by ({}) ({name})", labels.join(", "))
    }
}

/// `sum by (quantile[, labels...]) (name)` -- shows whatever quantiles the
/// summary actually emits, without assuming specific quantile values (which the
/// schema does not know).
fn summary_quantiles_query(name: &str, labels: &[String]) -> String {
    let mut by = vec!["quantile".to_owned()];
    by.extend(labels.iter().cloned());
    format!("sum by ({}) ({name})", by.join(", "))
}

/// `sum(rate(name_bucket[..])) by (<bucket-label>[, labels...])` -- the bucket
/// grouping a heatmap and the quantile share.
fn buckets_by(name: &str, labels: &[String], dialect: QueryDialect) -> String {
    let mut by = vec![dialect.bucket_label().to_owned()];
    by.extend(labels.iter().cloned());
    format!(
        "sum(rate({name}_bucket[$__rate_interval])) by ({})",
        by.join(", ")
    )
}

/// `sum(name_bucket) by (le[, labels...])` -- an *instantaneous* bucket grouping
/// for a gauge histogram.
///
/// Unlike a cumulative histogram, a gauge histogram's bucket counts reflect a
/// current population and can decrease, so `rate()` (which assumes a monotonic
/// counter) must not be applied. Gauge histograms are also always labelled `le`
/// (the renderer emits `le` directly), so there is no `vmrange` form to convert
/// and the query is dialect-independent in shape.
fn gauge_buckets_by(name: &str, labels: &[String]) -> String {
    let mut by = vec!["le".to_owned()];
    by.extend(labels.iter().cloned());
    format!("sum({name}_bucket) by ({})", by.join(", "))
}

fn histogram_quantile_query(
    name: &str,
    labels: &[String],
    quantile: &str,
    dialect: QueryDialect,
) -> String {
    format!(
        "histogram_quantile({quantile}, {})",
        buckets_by(name, labels, dialect)
    )
}

fn heatmap_query(name: &str, labels: &[String], dialect: QueryDialect) -> String {
    let buckets = buckets_by(name, labels, dialect);
    match dialect {
        // Grafana heatmaps understand `le` directly.
        QueryDialect::PromQl => buckets,
        // `vmrange` must be converted to `le` for Grafana heatmaps.
        QueryDialect::MetricsQl => format!("prometheus_buckets({buckets})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema() -> MetricSchema {
        let mut schema = MetricSchema::new();
        schema.add_family("requests", MetricType::Counter, &[]);
        schema.add_family("queue_depth", MetricType::Gauge, &[("service", "api")]);
        schema.add_family("build", MetricType::Info, &[("version", "1")]);
        schema.add_family("state", MetricType::StateSet, &[("state", "running")]);
        schema.add_family("latency_seconds", MetricType::Histogram, &[("route", "/a")]);
        schema.add_family("passthrough", MetricType::Unknown, &[("src", "legacy")]);
        schema.add_family(
            "current_items",
            MetricType::GaugeHistogram,
            &[("kind", "a")],
        );
        schema.add_family("legacy_latency", MetricType::Summary, &[("svc", "api")]);
        schema
    }

    #[test]
    fn promql_queries_group_histograms_by_le() {
        let queries = schema().queries(QueryDialect::PromQl);
        assert!(
            queries
                .iter()
                .any(|q| q.expr == "sum(rate(requests_total[$__rate_interval]))")
        );
        assert!(
            queries
                .iter()
                .any(|q| q.expr == "sum by (service) (queue_depth)")
        );
        // An Unknown family is treated as a plain untyped scalar: a plain
        // instant selector, the same shape as a gauge.
        assert!(
            queries
                .iter()
                .any(|q| q.expr == "sum by (src) (passthrough)")
        );
        assert!(queries.iter().any(|q| {
            q.expr
                == "histogram_quantile(0.99, sum(rate(latency_seconds_bucket[$__rate_interval])) by (le, route))"
        }));
        // Heatmap seed groups by le, no prometheus_buckets wrapper.
        assert!(queries.iter().any(|q| {
            q.title == "latency_seconds heatmap"
                && q.expr == "sum(rate(latency_seconds_bucket[$__rate_interval])) by (le, route)"
        }));
        // A gauge histogram is bucketed by `le` like a histogram, but its bucket
        // counts are a current population (they can decrease), so quantiles use
        // an *instantaneous* `sum(..._bucket)` -- never `rate()`.
        assert!(queries.iter().any(|q| {
            q.expr == "histogram_quantile(0.99, sum(current_items_bucket) by (le, kind))"
        }));
        assert!(queries.iter().any(|q| {
            q.title == "current_items distribution"
                && q.expr == "sum(current_items_bucket) by (le, kind)"
        }));
        assert!(queries.iter().all(|q| q.dialect == QueryDialect::PromQl));
    }

    #[test]
    fn summary_emits_a_single_generic_quantile_query() {
        let queries = schema().queries(QueryDialect::PromQl);
        let summary: Vec<_> = queries
            .iter()
            .filter(|q| q.title.starts_with("legacy_latency"))
            .collect();
        // A summary produces one generic query grouping by `quantile` (plus the
        // family's labels): it surfaces whatever quantiles the summary actually
        // emits, rather than inventing p50/p95/p99 selectors the schema can't know.
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].expr, "sum by (quantile, svc) (legacy_latency)");
        assert_eq!(summary[0].purpose, "Reported summary quantiles");
        // No invented concrete quantile selectors anywhere: the schema knows the
        // `quantile` label *name*, not the actual quantile *values* a summary emits.
        assert!(queries.iter().all(|q| !q.expr.contains("quantile=\"")));
    }

    #[test]
    fn metricsql_queries_group_histograms_by_vmrange_and_wrap_heatmaps() {
        let queries = schema().queries(QueryDialect::MetricsQl);
        assert!(queries.iter().any(|q| {
            q.expr
                == "histogram_quantile(0.95, sum(rate(latency_seconds_bucket[$__rate_interval])) by (vmrange, route))"
        }));
        assert!(queries.iter().any(|q| {
            q.title == "latency_seconds heatmap"
                && q.expr
                    == "prometheus_buckets(sum(rate(latency_seconds_bucket[$__rate_interval])) by (vmrange, route))"
        }));
        // Non-histogram queries are dialect-independent in form.
        assert!(
            queries
                .iter()
                .any(|q| q.expr == "sum(rate(requests_total[$__rate_interval]))")
        );
    }
}
