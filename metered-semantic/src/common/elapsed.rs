//! A duration metric backed by a cumulative bucket histogram, with exemplars.

use crate::metric::{Armed, Measure, Recorder};
use metered::bucket_histogram::{
    BucketHistogram, Buckets, ExemplarSource, HistogramSnapshot, NoExemplars,
};
use metered::handle::Handle;
use std::fmt;
use std::time::Instant;

/// A duration metric backed by a cumulative [`BucketHistogram`], with pluggable
/// exemplars.
///
/// Unlike a pre-computed HDR summary, `Elapsed`
/// records elapsed time into aggregatable `le` buckets in seconds and can
/// attach an exemplar to the landing bucket on every observation. The exemplar
/// labels come from `S` (an [`ExemplarSource`]); the observed value is filled in
/// automatically. Use the default `Elapsed` for no exemplars, or `Elapsed<MySource>`
/// to mint them from the active trace.
///
/// The elapsed time (and exemplar) is recorded on normal completion *and* on
/// abort (panic / early-exit / async cancellation).
pub struct Elapsed<S: ExemplarSource = NoExemplars> {
    histogram: Handle<BucketHistogram>,
    source: S,
}

/// Runtime configuration for [`Elapsed`].
#[derive(Clone, Debug)]
pub struct ElapsedConfig<S: ExemplarSource = NoExemplars> {
    /// Histogram bucket boundaries.
    pub buckets: Buckets,
    /// Exemplar source used for each observation.
    pub exemplar_source: S,
}

impl<S: ExemplarSource> Elapsed<S> {
    /// Builds an elapsed-time metric from explicit bucket and exemplar configuration.
    pub fn with_config(config: ElapsedConfig<S>) -> Self {
        Elapsed {
            histogram: Handle::new(BucketHistogram::new(config.buckets)),
            source: config.exemplar_source,
        }
    }
}

impl<S: ExemplarSource + Default> Elapsed<S> {
    /// Builds an elapsed-time metric over the given bucket boundaries.
    pub fn with_buckets(buckets: Buckets) -> Self {
        Elapsed::with_config(ElapsedConfig {
            buckets,
            exemplar_source: S::default(),
        })
    }
}

impl<S: ExemplarSource + Default> Default for Elapsed<S> {
    fn default() -> Self {
        Elapsed::with_buckets(Buckets::seconds_default())
    }
}

impl<S: ExemplarSource> Elapsed<S> {
    /// Takes a cumulative snapshot of the underlying histogram.
    pub fn snapshot(&self) -> HistogramSnapshot {
        self.histogram.snapshot()
    }
}

impl<S: ExemplarSource> Measure for Elapsed<S> {
    type Recorder = ElapsedRecorder<S>;

    fn enter(&self) -> ElapsedRecorder<S> {
        ElapsedRecorder {
            histogram: self.histogram.share(),
            start: Instant::now(),
            source: self.source.clone(),
            armed: Armed::new(),
        }
    }
}

/// Recorder for [`Elapsed`]: records elapsed seconds -- with an exemplar from `S`,
/// if any -- on completion or abort.
pub struct ElapsedRecorder<S: ExemplarSource> {
    histogram: Handle<BucketHistogram>,
    start: Instant,
    source: S,
    armed: Armed,
}

impl<S: ExemplarSource> ElapsedRecorder<S> {
    fn record(&self) {
        let seconds = self.start.elapsed().as_secs_f64();
        match self.source.exemplar() {
            Some(mut exemplar) => {
                // The exemplar refers to this observation, so its value is the
                // observed duration regardless of what the source set.
                exemplar.value = seconds;
                self.histogram.observe_with_exemplar(seconds, exemplar);
            }
            None => {
                self.histogram.observe(seconds);
            }
        }
    }
}

impl<S: ExemplarSource, R> Recorder<R> for ElapsedRecorder<S> {
    fn complete(&mut self, _result: &R) {
        if self.armed.fire() {
            self.record();
        }
    }
}

impl<S: ExemplarSource> Drop for ElapsedRecorder<S> {
    fn drop(&mut self) {
        if self.armed.fire() {
            self.record();
        }
    }
}

impl<S: ExemplarSource> fmt::Debug for Elapsed<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let snapshot = self.histogram.snapshot();
        f.debug_struct("Elapsed")
            .field("sum", &snapshot.sum)
            .field("count", &snapshot.count)
            .finish()
    }
}
