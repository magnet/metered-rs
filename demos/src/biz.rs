use metered::{metered, HitCount, Throughput};

#[derive(Default, Debug, serde::Serialize)]
pub struct Biz {
    pub(crate) metrics: BizMetrics,
}

#[metered(registry = BizMetrics)]
#[measure([HitCount, Throughput])]
impl Biz {
    // This is measured with an Throughput metric (TPS)
    #[measure]
    pub fn biz(&self) {
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 200);
        std::thread::sleep(delay);
    }
}
