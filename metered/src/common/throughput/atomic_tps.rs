use super::{tx_per_sec::TxPerSec, RecordThroughput};
use crate::{
    clear::Clear,
    hdr_histogram::HdrHistogram,
    time_source::{Instant, StdInstant},
};
use parking_lot::Mutex;
use serde::{Serialize, Serializer};

/// Thread-safe implementation of [`super::RecordThroughput`]. It uses a `Mutex` to wrap
/// `TxPerSec`.
pub struct AtomicTxPerSec<T: Instant = StdInstant> {
    /// The inner mutex protecting the `TxPerSec` value holding the histogram
    pub inner: Mutex<TxPerSec<T>>,
}

impl<T: Instant> AtomicTxPerSec<T> {
    /// Returns a cloned snapshot of the inner histogram.
    pub fn histogram(&self) -> HdrHistogram {
        self.inner.lock().hdr_histogram.clone()
    }
}

impl<T: Instant> RecordThroughput for AtomicTxPerSec<T> {
    #[inline]
    fn on_result(&self) {
        self.inner.lock().on_result()
    }
}

impl<T: Instant> Default for AtomicTxPerSec<T> {
    fn default() -> Self {
        AtomicTxPerSec {
            inner: Mutex::new(TxPerSec::default()),
        }
    }
}

impl<T: Instant> Clear for AtomicTxPerSec<T> {
    fn clear(&self) {
        self.inner.lock().clear();
    }
}

impl<T: Instant> Serialize for AtomicTxPerSec<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let inner = self.inner.lock();
        Serialize::serialize(&*inner, serializer)
    }
}

use std::{fmt, fmt::Debug};
impl<T: Instant> Debug for AtomicTxPerSec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let inner = self.inner.lock();
        write!(f, "{:?}", &*inner)
    }
}
