use super::RecordThroughput;
use crate::{
    clear::Clear,
    hdr_histogram::HdrHistogram,
    time_source::{Instant, StdInstant},
};
use serde::{Serialize, Serializer};

/// Non-thread safe implementation of `RecordThroughput`. Use as
/// `RefCell<TxPerSec<T>>`.
pub struct TxPerSec<T: Instant = StdInstant> {
    /// The inner histogram
    pub hdr_histogram: HdrHistogram,
    start_time: Option<T>,
    last_window: u64,
    count: u64,
    time_source: std::marker::PhantomData<T>,
}

impl<T: Instant> Default for TxPerSec<T> {
    fn default() -> Self {
        TxPerSec {
            // Bound at 100K TPS, higher values will be saturated...
            // TODO: make this configurable :)
            hdr_histogram: HdrHistogram::with_bound(100_000),
            start_time: None,
            last_window: 0,
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

impl<T: Instant> Clear for std::cell::RefCell<TxPerSec<T>> {
    fn clear(&self) {
        self.borrow_mut().clear();
    }
}

impl<T: Instant> TxPerSec<T> {
    /// Record previous count if the 1-sec window has closed and advance time
    /// window
    fn update(&mut self) {
        if let Some(ref start_time) = self.start_time {
            let elapsed = start_time.elapsed_time();
            let this_window = elapsed / T::ONE_SEC;
            if this_window > self.last_window {
                // Record this window
                self.hdr_histogram.record(self.count);
                self.count = 0;

                // Record windows with no samples
                let empty_windows = this_window - self.last_window - 1;
                if empty_windows > 0 {
                    self.hdr_histogram.record_n(0, empty_windows)
                }

                // Advance window
                self.last_window = this_window;
            }
        } else {
            // Set first window start time
            self.start_time = Some(T::now());
        };
    }
    pub(crate) fn on_result(&mut self) {
        self.update();
        self.count += 1;
    }

    pub(crate) fn clear(&mut self) {
        self.hdr_histogram.clear();
        self.start_time = None;
        self.last_window = 0;
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

use std::{fmt, fmt::Debug};
impl<T: Instant> Debug for TxPerSec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", &self.hdr_histogram)
    }
}
