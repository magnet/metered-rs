use metered_semantic::{metered, HitCount};

#[derive(Default, Debug)]
pub struct Biz {
    pub(crate) metrics: BizMetrics,
}

#[metered(registry = BizMetrics)]
#[measure(HitCount)]
impl Biz {
    #[measure]
    pub fn biz(&self) {
        let delay = std::time::Duration::from_millis(rand::random::<u64>() % 200);
        std::thread::sleep(delay);
    }
}
