use super::RecordThroughput;
use crate::hdr_histogram::HdrHistogram;
use crate::time_source::Instant;
use serde::{Serialize, Serializer};

pub struct TxPerSec<T: Instant> {
    hdr_histogram: HdrHistogram,
    last: Option<T>,
    count: u64,
    time_source: std::marker::PhantomData<T>,
}

impl<T: Instant> Default for TxPerSec<T> {
    fn default() -> Self {
        TxPerSec {
            // Bound at 100K TPS, higher values will be saturated...
            // TODO: make this configurable :)
            hdr_histogram: HdrHistogram::with_bound(100_000),
            last: None,
            count: 0,
            time_source: std::marker::PhantomData,
        }
    }
}

impl<T: Instant> RecordThroughput for std::cell::RefCell<TxPerSec<T>> {
    #[inline]
    fn on_result(&self) {
        self.borrow_mut().on_result()
    }
}

impl<T: Instant> TxPerSec<T> {
    pub fn on_result(&mut self) {
        // Record previous count if the 1-sec window has closed
        if let Some(ref last) = self.last {
            let elapsed = last.elapsed_time();
            if elapsed > T::ONE_SEC {
                self.hdr_histogram.record(self.count);
                self.count = 0;
                self.last = Some(T::now());
            }
        } else {
            // Start a new window
            self.last = Some(T::now());
        };

        self.count += 1;
    }

    pub fn clear(&mut self) {
        self.hdr_histogram.clear();
        self.last = None;
        self.count = 0;
    }
}

impl<T: Instant> Serialize for TxPerSec<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(&self.hdr_histogram, serializer)
    }
}

use std::fmt;
use std::fmt::Debug;
impl<T: Instant> Debug for TxPerSec<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", &self.hdr_histogram)
    }
}
