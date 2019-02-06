use crate::clear::Clear;
use crate::metric::Histogram;
use atomic_refcell::AtomicRefCell;
use serde::{Serialize, Serializer};

pub struct AtomicHdrHistogram {
    inner: AtomicRefCell<HdrHistogram>,
}


impl Histogram for AtomicHdrHistogram {
    fn with_bound(max_bound: u64) -> Self {
        let histo = HdrHistogram::with_bound(max_bound);
        let inner = AtomicRefCell::new(histo);
        AtomicHdrHistogram { inner }
    }

    fn record(&self, value: u64) {
        self.inner.borrow_mut().record(value);
    }
}

impl Clear for AtomicHdrHistogram {
    fn clear(&self) {
        self.inner.borrow_mut().clear();
    }
}

impl Serialize for AtomicHdrHistogram {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use std::ops::Deref;
        let inner = self.inner.borrow();
        let inner = inner.deref();
        Serialize::serialize(inner, serializer)
    }
}

use std::fmt;
use std::fmt::Debug;
impl Debug for AtomicHdrHistogram {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let histo = self.inner.borrow();
        write!(f, "AtomicHdrHistogram {{ {:?} }}", &histo)
    }
}

pub struct HdrHistogram {
    histo: hdrhistogram::Histogram<u64>,
}

impl HdrHistogram {
    pub fn with_bound(max_bound: u64) -> Self {
        let histo = hdrhistogram::Histogram::<u64>::new_with_bounds(1, max_bound, 2)
            .expect("Could not instantiate HdrHistogram");

        HdrHistogram { histo }
    }

    pub fn record(&mut self, value: u64) {
        // All recordings will be saturating, that is, a value higher than 5 minutes
        // will be replace by 5 minutes...
        self.histo.saturating_record(value);
    }

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
        tup.serialize_entry("99.9ile", &ile(99.9))?;
        tup.serialize_entry("99.99ile", &ile(99.99))?;
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
