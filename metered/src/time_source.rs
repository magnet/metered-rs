
pub trait Instant {
    fn now() -> Self;
    fn elapsed_millis(&self) -> u64;
}


#[derive(Debug, Clone)]
pub struct StdInstant(std::time::Instant);
impl Instant for StdInstant {
    fn now() -> Self {
        StdInstant(std::time::Instant::now())
    }

    fn elapsed_millis(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}
