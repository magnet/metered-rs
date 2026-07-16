use metered::{MetricSchema, MetricTreeExt};
use order_service_demo::app::App;

#[test]
fn demo_schema_is_the_static_contract() {
    let app = App::demo();
    let schema = app.schema();

    assert_eq!(
        render_schema(&schema),
        r#"| Family | Type | Labels | Unit | Help |
| --- | --- | --- | --- | --- |
| db_client_duration_seconds | histogram | db_operation, service | seconds | Database queries |
| db_client_overflowed_spans | counter | service |  | Span closes folded into the all-_OTHER series because the metric was at its max_series bound |
| db_client_requests | counter | db_operation, service |  | Database queries |
| db_pool_idle | gauge | service |  | Pool connections sitting idle |
| db_pool_in_use | gauge | service |  | Pool connections currently checked out |
| db_pool_utilization | gauge | service |  | Fraction of pool connections in use |
| db_query_errors | counter | service |  | DB write errors |
| order_service_jobs_queue_depth | gauge |  |  | Background jobs waiting in the queue |
| order_service_jobs_run_duration_seconds | histogram | outcome | seconds | Background job runs |
| order_service_jobs_run_overflowed_spans | counter |  |  | Span closes folded into the all-_OTHER series because the metric was at its max_series bound |
| order_service_jobs_run_requests | counter | outcome |  | Background job runs |
| order_service_orders_cache_entries | gauge |  |  | Orders held in the in-memory cache |
| order_service_orders_create_duration_seconds | histogram | category, channel | seconds | Order creation operations |
| order_service_orders_create_overflowed_spans | counter |  |  | Span closes folded into the all-_OTHER series because the metric was at its max_series bound |
| order_service_orders_create_requests | counter | category, channel |  | Order creation operations |
| order_service_orders_created | counter | category, channel |  | Orders created, by category and channel |
| order_service_payments_failures | counter | rail |  | Failed settlements on the rail |
| order_service_payments_in_flight | gauge | rail |  | Settlements in flight on the rail |
| order_service_payments_settlements | counter | rail |  | Payments settled over the rail |
| rpc_server_duration_seconds | histogram | rpc_method, rpc_status, service | seconds | RPC server calls handled by the transport layer |
| rpc_server_requests | counter | rpc_method, rpc_status, service |  | RPC server calls handled by the transport layer |
| service | info | region, service_name, service_version |  | Service build metadata |"#
    );
}

fn render_schema(schema: &MetricSchema) -> String {
    let rows = schema
        .families()
        .iter()
        .map(|family| {
            format!(
                "| {} | {} | {} | {} | {} |",
                family.name,
                family.metric_type.as_str(),
                family.labels.join(", "),
                family
                    .unit
                    .as_ref()
                    .map(metered::Unit::as_str)
                    .unwrap_or(""),
                family
                    .help
                    .as_ref()
                    .map(metered::Help::as_str)
                    .unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("| Family | Type | Labels | Unit | Help |\n| --- | --- | --- | --- | --- |\n{rows}")
}
