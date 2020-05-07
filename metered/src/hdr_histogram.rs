//! A module providing thread-safe and unsynchronized implementations for Histograms, based on HdrHistogram.

use crate::clear::Clear;
use crate::metric::Histogram;
use parking_lot::Mutex;
use serde::{Serialize, Serializer};

/// A thread-safe implementation of HdrHistogram
pub struct AtomicHdrHistogram {
    inner: Mutex<HdrHistogram>,
}

impl Histogram for AtomicHdrHistogram {
    fn with_bound(max_bound: u64) -> Self {
        let histo = HdrHistogram::with_bound(max_bound);
        let inner = Mutex::new(histo);
        AtomicHdrHistogram { inner }
    }

    fn record(&self, value: u64) {
        self.inner.lock().record(value);
    }
}

impl Clear for AtomicHdrHistogram {
    fn clear(&self) {
        self.inner.lock().clear();
    }
}

impl Serialize for AtomicHdrHistogram {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use std::ops::Deref;
        let inner = self.inner.lock();
        let inner = inner.deref();
        Serialize::serialize(inner, serializer)
    }
}

use std::fmt;
use std::fmt::Debug;
impl Debug for AtomicHdrHistogram {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let histo = self.inner.lock();
        write!(f, "AtomicHdrHistogram {{ {:?} }}", &*histo)
    }
}

/// An High-Dynamic Range Histogram
///
/// HdrHistograms can record and analyze sampled data in low-latency applications. Read more about HDR Histograms on [http://hdrhistogram.org/](http://hdrhistogram.org/)
///
/// This structure uses the `hdrhistogram` crate under the hood.
pub struct HdrHistogram {
    histo: hdrhistogram::Histogram<u64>,
}

impl HdrHistogram {
    /// Instantiates a new HdrHistogram with a max_bound
    ///
    /// For instance, a max_bound of 60 * 60 * 1000 will allow to record durations varying from 1 millisecond to 1 hour.
    pub fn with_bound(max_bound: u64) -> Self {
        let histo = hdrhistogram::Histogram::<u64>::new_with_bounds(1, max_bound, 2)
            .expect("Could not instantiate HdrHistogram");

        HdrHistogram { histo }
    }

    /// Records a value to the histogram
    ///
    /// This is a saturating record: if the value is higher than `max_bound`, max_bound will be recorded instead.
    pub fn record(&mut self, value: u64) {
        // All recordings will be saturating
        self.histo.saturating_record(value);
    }

    /// Clears the values of the histogram
    pub fn clear(&mut self) {
        self.histo.reset();
    }
}

impl Serialize for HdrHistogram {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let hdr = &self.histo;
        let ile = |v| hdr.value_at_percentile(v);
        use serde::ser::SerializeMap;

        let mut tup = serializer.serialize_map(Some(10))?;

        tup.serialize_entry("samples", &hdr.len())?;
        tup.serialize_entry("min", &hdr.min())?;
        tup.serialize_entry("max", &hdr.max())?;
        tup.serialize_entry("mean", &hdr.mean())?;
        tup.serialize_entry("stdev", &hdr.stdev())?;
        tup.serialize_entry("90%ile", &ile(90.0))?;
        tup.serialize_entry("95%ile", &ile(95.0))?;
        tup.serialize_entry("99%ile", &ile(99.0))?;
        tup.serialize_entry("99.9%ile", &ile(99.9))?;
        tup.serialize_entry("99.99%ile", &ile(99.99))?;
        tup.end()
    }
}

impl Debug for HdrHistogram {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let hdr = &self.histo;
        let ile = |v| hdr.value_at_percentile(v);
        write!(
            f,
            "HdrHistogram {{ 
            samples: {}, min: {}, max: {}, mean: {}, stdev: {},
            90%ile = {}, 95%ile = {}, 99%ile = {}, 99.9%ile = {}, 99.99%ile = {} }}",
            hdr.len(),
            hdr.min(),
            hdr.max(),
            hdr.mean(),
            hdr.stdev(),
            ile(90.0),
            ile(95.0),
            ile(99.0),
            ile(99.9),
            ile(99.99)
        )
    }
}

use std::cell::RefCell;

impl Histogram for RefCell<HdrHistogram> {
    fn with_bound(max_value: u64) -> Self {
        RefCell::new(HdrHistogram::with_bound(max_value))
    }

    fn record(&self, value: u64) {
        self.borrow_mut().record(value);
    }
}

impl Clear for RefCell<HdrHistogram> {
    fn clear(&self) {
        self.borrow_mut().clear();
    }
}