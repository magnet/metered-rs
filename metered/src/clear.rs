//! A module providing a Clear trait which signals metrics to clear their state if applicable.

pub trait Clear {
    fn clear(&self);
}
