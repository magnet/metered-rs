#[test]
fn view_accepts_typed_name_help_and_unit() {
    use metered::{AsGauge, Help, MetricTreeView, Name, Unit};
    use metered_om::OpenMetricsViewExt;
    use std::sync::atomic::AtomicU64;

    struct App {
        queue_depth: AsGauge<AtomicU64>,
    }

    let app = App {
        queue_depth: AsGauge::from(AtomicU64::new(3)),
    };

    let mut view = MetricTreeView::with_prefix(Name::from("app"));
    view.register(
        metered::entry::metric(Name::from("queue_depth"))
            .select(|app: &App| &app.queue_depth)
            .help(Help::from("Queue depth"))
            .unit(Unit::Items),
    );

    let text = view.encode_to_string(&app).unwrap();
    assert!(text.contains("# HELP app_queue_depth Queue depth"));
    // `items` is not an `_`-separated suffix of `app_queue_depth`, so the
    // non-conformant `# UNIT` line is suppressed to keep Prometheus from
    // rejecting the scrape; the family still renders.
    assert!(!text.contains("# UNIT app_queue_depth"));
    assert!(text.contains("app_queue_depth 3"));
}

#[test]
fn view_register_uses_self_describing_entries() {
    use metered::entry::{counter, gauge, info, metric, tree};
    use metered::{Help, InfoMetric, MetricTree, MetricTreeView, Name, Unit};
    use metered_om::OpenMetricsViewExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[derive(Default, MetricTree)]
    struct CatalogMetrics {
        #[metric(counter)]
        refreshes: AtomicU64,
    }

    struct Catalog {
        metrics: CatalogMetrics,
        queue_depth: AtomicU64,
    }

    struct App {
        info: InfoMetric,
        requests: AtomicU64,
        catalog: Arc<Catalog>,
    }

    // Mounted under the "catalog" name segment below, so the view itself
    // carries no prefix: the mount segment names it (there is no hidden
    // "prefix equals mount name" dedup collapsing a doubled segment).
    fn catalog_view() -> MetricTreeView<'static, Catalog> {
        let mut view = MetricTreeView::new();
        view.register(
            gauge("queue_depth")
                .select(|catalog: &Catalog| &catalog.queue_depth)
                .help("Catalog queue depth")
                .unit(Unit::Items),
        );
        view.register(
            metric("metrics")
                .select(|catalog: &Catalog| &catalog.metrics)
                .help("Catalog explicit metrics"),
        );
        view
    }

    let app = App {
        info: InfoMetric::new([("version", "1")]),
        requests: AtomicU64::new(5),
        catalog: Arc::new(Catalog {
            metrics: CatalogMetrics::default(),
            queue_depth: AtomicU64::new(9),
        }),
    };
    app.catalog.metrics.refreshes.store(2, Ordering::Relaxed);

    let mut view = MetricTreeView::with_prefix(Name::from("app"));
    view.register(
        info("service")
            .select(|app: &App| &app.info)
            .help(Help::from("Service metadata")),
    );
    view.register(
        counter("requests")
            .select(|app: &App| &app.requests)
            .help("Total requests")
            .unit(Unit::Requests),
    );
    view.register(
        tree("catalog")
            .select(|app: &App| app.catalog.as_ref())
            .view(catalog_view()),
    );

    let text = view.encode_to_string(&app).unwrap();
    assert!(text.contains("# TYPE app_requests counter"));
    assert!(text.contains("# UNIT app_requests requests"));
    assert!(text.contains("app_requests_total 5"));
    assert!(text.contains("# TYPE app_service info"));
    assert!(text.contains("app_service_info{version=\"1\"} 1"));
    assert!(text.contains("# TYPE app_catalog_queue_depth gauge"));
    assert!(text.contains("app_catalog_queue_depth 9"));
    assert!(text.contains("# TYPE app_catalog_metrics_refreshes counter"));
    assert!(text.contains("app_catalog_metrics_refreshes_total 2"));
}

#[test]
fn typed_entries_infer_context_and_chain_metadata() {
    use metered::entry::{counter, gauge_value, info, metric};
    use metered::{InfoMetric, MetricTree, MetricTreeView, Unit};
    use metered_om::OpenMetricsViewExt;
    use std::sync::atomic::AtomicU64;
    use std::sync::Mutex;

    #[derive(Default, MetricTree)]
    struct DbMetrics {
        #[metric(gauge, help = "Idle pool connections")]
        pool_idle: AtomicU64,
        #[metric(counter, help = "Query errors")]
        query_errors: AtomicU64,
    }

    struct Db {
        metrics: DbMetrics,
    }

    struct App {
        info: InfoMetric,
        requests: AtomicU64,
        cache: Mutex<Vec<u64>>,
        db: Db,
    }

    fn db_view() -> MetricTreeView<'static, Db> {
        let mut view = MetricTreeView::new();
        view.register(
            metric("pool")
                .select(|db: &Db| &db.metrics)
                .help("In-memory DB pool metrics"),
        );
        view
    }

    let app = App {
        info: InfoMetric::new([("version", "1")]),
        requests: AtomicU64::new(5),
        cache: Mutex::new(vec![1, 2, 3]),
        db: Db {
            metrics: DbMetrics::default(),
        },
    };
    metered::Counter::incr(&app.db.metrics.query_errors);

    // Selector closures (`-> &T`) need the arg annotation; the computed
    // `gauge_value` read (`-> i64`) infers it.
    let mut view = MetricTreeView::with_prefix("app");
    view.register(
        info("service")
            .select(|app: &App| &app.info)
            .help("Service metadata"),
    );
    view.register(
        counter("requests")
            .select(|app: &App| &app.requests)
            .help("Total requests")
            .unit(Unit::Requests),
    );
    view.register(
        gauge_value("cache_entries")
            .read(|app: &App| app.cache.lock().unwrap().len() as i64)
            .help("Cache entries"),
    );
    view.tree("db", |app: &App| &app.db, db_view());

    let text = view.encode_to_string(&app).unwrap();
    assert!(text.contains("# TYPE app_requests counter"));
    assert!(text.contains("# UNIT app_requests requests"));
    assert!(text.contains("app_requests_total 5"));
    assert!(text.contains("# TYPE app_service info"));
    assert!(text.contains("# TYPE app_cache_entries gauge"));
    assert!(text.contains("app_cache_entries 3"));
    assert!(text.contains("# HELP app_cache_entries Cache entries"));
    // Per-field derive metadata flows through the fluent `metric` selector.
    assert!(text.contains("# TYPE app_db_pool_pool_idle gauge"));
    assert!(text.contains("# TYPE app_db_pool_query_errors counter"));
    assert!(text.contains("app_db_pool_query_errors_total 1"));
}

#[test]
fn metrics_view_trait_and_subtree_compose_components() {
    use metered::{MetricTreeView, MetricsView};
    use metered_om::OpenMetricsViewExt;
    use std::sync::atomic::AtomicU64;

    struct Worker {
        runs: AtomicU64,
    }
    impl MetricsView for Worker {
        fn metrics_view() -> MetricTreeView<'static, Self> {
            let mut view = MetricTreeView::new();
            view.register(
                metered::entry::counter("runs")
                    .select(|w: &Worker| &w.runs)
                    .help("Worker runs"),
            );
            view
        }
    }

    struct App {
        worker: Worker,
    }

    let app = App {
        worker: Worker {
            runs: AtomicU64::new(4),
        },
    };

    let mut view = MetricTreeView::with_prefix("app");
    // No free `worker_view()`; the component owns its layout.
    view.subtree("worker", |app: &App| &app.worker);

    let text = view.encode_to_string(&app).unwrap();
    assert!(text.contains("# TYPE app_worker_runs counter"));
    assert!(text.contains("app_worker_runs_total 4"));
}

#[test]
fn collection_exposes_dynamic_subservices_with_a_key_label() {
    use metered::{MetricTreeView, MetricsView};
    use metered_om::{OpenMetricsDocument, OpenMetricsViewExt};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicI64, AtomicU64};
    use std::sync::RwLock;

    // Metrics live *in* the dynamic domain objects, not a central Family.
    struct Upstream {
        in_flight: AtomicI64,
        requests: AtomicU64,
    }
    impl MetricsView for Upstream {
        fn metrics_view() -> MetricTreeView<'static, Self> {
            let mut view = MetricTreeView::new();
            view.register(
                metered::entry::gauge("in_flight")
                    .select(|u: &Upstream| &u.in_flight)
                    .help("In-flight requests"),
            );
            view.register(
                metered::entry::counter("requests")
                    .select(|u: &Upstream| &u.requests)
                    .help("Requests sent"),
            );
            view
        }
    }

    struct Proxy {
        upstreams: RwLock<HashMap<String, Upstream>>,
    }

    let proxy = Proxy {
        upstreams: RwLock::new(HashMap::new()),
    };
    {
        let mut map = proxy.upstreams.write().unwrap();
        map.insert(
            "payments".to_owned(),
            Upstream {
                in_flight: AtomicI64::new(2),
                requests: AtomicU64::new(10),
            },
        );
        map.insert(
            "inventory".to_owned(),
            Upstream {
                in_flight: AtomicI64::new(0),
                requests: AtomicU64::new(7),
            },
        );
    }

    let mut view = MetricTreeView::with_prefix("proxy");
    // The set is dynamic and the metrics live in the elements; `iterate` owns the
    // lock scope and emits one (key, element) at a time.
    view.each(
        "upstream",
        Upstream::metrics_view(),
        |proxy: &Proxy, out| {
            for (name, upstream) in proxy.upstreams.read().unwrap().iter() {
                out.emit(name, upstream);
            }
        },
    );

    let text = view.encode_to_string(&proxy).unwrap();
    let doc = OpenMetricsDocument::parse(&text).unwrap();

    let payments_in_flight = doc
        .samples_named("proxy_in_flight")
        .into_iter()
        .find(|s| s.label("upstream") == Some("payments"))
        .unwrap();
    assert_eq!(payments_in_flight.value, "2");

    let inventory_requests = doc
        .samples_named("proxy_requests_total")
        .into_iter()
        .find(|s| s.label("upstream") == Some("inventory"))
        .unwrap();
    assert_eq!(inventory_requests.value, "7");
}

#[test]
fn each_schema_is_context_free_with_no_live_members() {
    use metered::{MetricTreeView, MetricType, MetricsView};
    use std::sync::atomic::AtomicU64;

    struct Shard {
        requests: AtomicU64,
    }
    impl MetricsView for Shard {
        fn metrics_view() -> MetricTreeView<'static, Self> {
            let mut view = MetricTreeView::new();
            view.register(
                metered::entry::counter("requests")
                    .select(|s: &Shard| &s.requests)
                    .help("Requests handled by a shard"),
            );
            view
        }
    }

    struct Router {
        shards: Vec<Shard>,
    }

    // No live members at all: the schema must still advertise the family and
    // its key label, because a scrape's shape may not depend on runtime state.
    let router = Router { shards: Vec::new() };
    let mut view = MetricTreeView::with_prefix("router");
    view.each("shard", Shard::metrics_view(), |router: &Router, out| {
        for (index, shard) in router.shards.iter().enumerate() {
            out.emit(&index.to_string(), shard);
        }
    });

    let schema = view.schema(&router);
    let family = schema
        .family("router_requests")
        .expect("family described even with zero members");
    assert_eq!(family.metric_type, MetricType::Counter);
    assert!(
        family.labels.iter().any(|label| label == "shard"),
        "the key label is declared context-free, got {:?}",
        family.labels
    );
}

#[test]
fn view_selectors_can_capture_local_state() {
    use metered::entry::gauge;
    use metered::{MetricTreeView, Unit};
    use metered_om::OpenMetricsViewExt;
    use std::sync::atomic::AtomicU64;

    struct App {
        primary_depth: AtomicU64,
        secondary_depth: AtomicU64,
    }

    let app = App {
        primary_depth: AtomicU64::new(1),
        secondary_depth: AtomicU64::new(7),
    };
    let use_secondary = true;

    let mut view = MetricTreeView::with_prefix("app");
    view.register(
        gauge("selected_depth")
            .select(move |app: &App| {
                if use_secondary {
                    &app.secondary_depth
                } else {
                    &app.primary_depth
                }
            })
            .help("Selected queue depth")
            .unit(Unit::Items),
    );

    let text = view.encode_to_string(&app).unwrap();
    assert!(text.contains("# TYPE app_selected_depth gauge"));
    assert!(text.contains("app_selected_depth 7"));
}

#[test]
fn projected_tree_under_each_is_flagged_by_validate() {
    use metered::entry::{counter, metric};
    use metered::{MetricTreeView, SchemaError};
    use std::sync::atomic::AtomicU64;

    // `io` is a metric *tree* reached by projection: under `each` its schema is
    // unreachable without a live member, so its values would silently diverge
    // from the declared scrape shape.
    struct Conn {
        connects: AtomicU64,
        io: AtomicU64,
    }
    struct Pool {
        conns: Vec<Conn>,
    }

    let mut element = MetricTreeView::new();
    element.register(counter("connects").select(|conn: &Conn| &conn.connects));
    element.register(metric("io").select(|conn: &Conn| &conn.io));

    let mut view = MetricTreeView::with_prefix("pool");
    view.each("conn", element, |pool: &Pool, out| {
        for (index, conn) in pool.conns.iter().enumerate() {
            out.emit(&index.to_string(), conn);
        }
    });

    let pool = Pool { conns: Vec::new() };
    let errors = view.schema(&pool).validate().unwrap_err();
    assert_eq!(
        errors,
        vec![SchemaError::UndescribedEntry {
            name: "pool_io".to_owned(),
        }],
        "the projected tree under `each` is flagged; the typed entry is not"
    );
}

#[test]
fn typed_entries_under_each_validate_clean() {
    use metered::entry::counter;
    use metered::MetricTreeView;
    use std::sync::atomic::AtomicU64;

    struct Conn {
        connects: AtomicU64,
    }
    struct Pool {
        conns: Vec<Conn>,
    }

    let mut element = MetricTreeView::new();
    element.register(counter("connects").select(|conn: &Conn| &conn.connects));

    let mut view = MetricTreeView::with_prefix("pool");
    view.each("conn", element, |pool: &Pool, out| {
        for (index, conn) in pool.conns.iter().enumerate() {
            out.emit(&index.to_string(), conn);
        }
    });

    let pool = Pool { conns: Vec::new() };
    view.schema(&pool)
        .validate()
        .expect("typed element entries declare a complete context-free schema");
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "an `each` element view declares no context-free schema")]
fn each_with_no_describable_entry_asserts_at_registration() {
    use metered::entry::metric;
    use metered::MetricTreeView;
    use std::sync::atomic::AtomicU64;

    struct Conn {
        io: AtomicU64,
    }
    struct Pool {
        conns: Vec<Conn>,
    }

    // Every entry is a runtime projection: the group would emit values with no
    // schema at all, which is a wiring bug caught at registration.
    let mut element = MetricTreeView::new();
    element.register(metric("io").select(|conn: &Conn| &conn.io));

    let mut view = MetricTreeView::with_prefix("pool");
    view.each("conn", element, |pool: &Pool, out| {
        for (index, conn) in pool.conns.iter().enumerate() {
            out.emit(&index.to_string(), conn);
        }
    });
}
