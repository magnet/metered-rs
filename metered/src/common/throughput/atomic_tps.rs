use super::tx_per_sec::TxPerSec;
use super::RecordThroughput;
use crate::clear::Clear;
use crate::time_source::{Instant, StdInstant};
use atomic_refcell::AtomicRefCell;
use serde::{Serialize, Serializer};

pub struct AtomicTxPerSec<T: Instant = StdInstant> {
    inner: AtomicRefCell<TxPerSec<T>>,
}

impl<T: Instant> RecordThroughput for AtomicTxPerSec<T> {
    #[inline]
    fn on_result(&self) {
        self.inner.borrow_mut().on_result()
    }
}

impl<T: Instant> Default for AtomicTxPerSec<T> {
    fn default() -> Self {
        AtomicTxPerSec {
            inner: AtomicRefCell::new(TxPerSec::default()),
        }
    }
}

impl<T: Instant> Clear for AtomicTxPerSec<T> {
    fn clear(&self) {
        self.inner.borrow_mut().clear();
    }
}

impl<T: Instant> Serialize for AtomicTxPerSec<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use std::ops::Deref;
        let inner = self.inner.borrow();
        let inner = inner.deref();
        Serialize::serialize(&inner, serializer)
    }
}

use std::fmt;
use std::fmt::Debug;
impl<T: Instant> Debug for AtomicTxPerSec<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use std::ops::Deref;

        let inner = self.inner.borrow();
        let inner = inner.deref();
        write!(f, "{:?}", inner)
    }
}
