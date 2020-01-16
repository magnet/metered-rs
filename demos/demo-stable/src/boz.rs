use metered::{metered, HitCount};

#[derive(Default, Debug, serde::Serialize)]
pub struct Boz {
    inc: i32,
    metrics: BozMetrics,
}

#[metered(registry = BozMetrics)]
impl Boz {

    #[measure(HitCount)]
    pub fn increment_once(&mut self) {
        self.inc +=1;
    }

    #[measure(HitCount)]
    pub fn increment_twice(&mut self) {
        self.increment_once();
        self.increment_once();
    }
}
