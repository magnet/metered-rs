use metered::{Counter, CounterSource, Gauge, GaugeSource, Info, InfoMetric, Labels, Registry};

struct ExternalCounter(std::sync::atomic::AtomicU64);

impl CounterSource for ExternalCounter {
    fn get(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Counter for ExternalCounter {
    fn incr_by(&self, n: u64) {
        self.0.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
    }
}

struct ExternalGauge(std::sync::atomic::AtomicI64);

impl GaugeSource for ExternalGauge {
    type Value = i64;

    fn get(&self) -> Self::Value {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Gauge for ExternalGauge {
    fn set(&self, value: Self::Value) {
        self.0.store(value, std::sync::atomic::Ordering::Relaxed);
    }

    fn add(&self, delta: Self::Value) {
        self.0
            .fetch_add(delta, std::sync::atomic::Ordering::Relaxed);
    }

    fn incr(&self) {
        self.add(1);
    }

    fn try_decr(&self) -> bool {
        self.0
            .fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |value| value.checked_sub(1),
            )
            .is_ok()
    }
}

struct ServiceInfo {
    service: String,
    version: String,
}

impl Info for ServiceInfo {
    fn labels(&self) -> Labels {
        Labels::from([
            ("service", self.service.as_str()),
            ("version", self.version.as_str()),
        ])
    }
}

#[test]
fn registry_registers_external_scalar_trait_values_with_fixed_metric_kinds() {
    let counter = ExternalCounter(std::sync::atomic::AtomicU64::new(0));
    counter.incr_by(7);
    let gauge = ExternalGauge(std::sync::atomic::AtomicI64::new(0));
    gauge.set(3);
    gauge.decr();
    let info = ServiceInfo {
        service: "orders".to_owned(),
        version: "1.2.3".to_owned(),
    };

    let mut registry = Registry::new();
    registry.register(
        metered::entry::counter("requests")
            .source(&counter)
            .help("Requests"),
    );
    registry.register(
        metered::entry::gauge("queue_depth")
            .source(&gauge)
            .help("Queue depth"),
    );
    registry.register(
        metered::entry::info("build")
            .source(&info)
            .help("Build info"),
    );

    let schema = registry.schema();
    assert_eq!(
        schema.family("requests").unwrap().metric_type,
        metered::MetricType::Counter
    );
    assert_eq!(
        schema.family("queue_depth").unwrap().metric_type,
        metered::MetricType::Gauge
    );
    assert_eq!(
        schema.family("build").unwrap().metric_type,
        metered::MetricType::Info
    );

    let values = registry.values();
    assert!(values
        .samples()
        .iter()
        .any(|s| s.name == "requests_total" && s.value.to_string() == "7"));
    assert!(values
        .samples()
        .iter()
        .any(|s| s.name == "queue_depth" && s.value.to_string() == "2"));
    assert!(values.samples().iter().any(|s| {
        s.name == "build_info"
            && s.value.to_string() == "1"
            && s.labels
                == vec![
                    ("service".to_owned(), "orders".to_owned()),
                    ("version".to_owned(), "1.2.3".to_owned()),
                ]
    }));
}

#[test]
fn info_metric_labels_are_runtime_mutable() {
    let info = InfoMetric::new([("version", "1.0.0")]);
    assert_eq!(info.labels().as_slice()[0].1, "1.0.0");

    info.set_labels(Labels::from([("version", "2.0.0"), ("channel", "blue")]));
    assert_eq!(
        info.labels().as_slice(),
        &[
            ("version".to_owned(), "2.0.0".to_owned()),
            ("channel".to_owned(), "blue".to_owned()),
        ]
    );
}
