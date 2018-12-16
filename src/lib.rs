use std::ops::Add;

pub trait Counter: Copy + Clone + Add<Self, Output = Self>   {
    type ValueType: From<u8> + Add<Self::ValueType, Output = Self::ValueType>;

    fn value(&self) -> Self::ValueType;
    fn value_mut(&mut self) -> &mut Self::ValueType;

    
    fn inc(&mut self) {
        *self.value_mut() = self.value() + Self::ValueType::from(1)
    }

    fn merge_with(&mut self, other: &Self) {
        *self = *self + *other
    }
}

impl Counter for u32 {
    type ValueType = u32;

    fn value(&self) -> u32 {
        *self
    }

    fn value_mut(&mut self) -> &mut u32 {
        self
    }
}

#[derive(Clone, Default)]
pub struct MetricRegistry {
    pub counters32: Vec<u32>,
}

impl MetricRegistry {
    pub fn new() -> MetricRegistry {
        Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn should_count_metrics() {
        let mut mr = MetricRegistry::new();
        mr.counters32.push(0);

        mr.counters32[0].inc();

        assert_eq!(mr.counters32[0], 1);
    }
}
