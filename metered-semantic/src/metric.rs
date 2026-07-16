//! Shared recording traits used by `measure!` and macro-generated code.
//!
//! These power the legacy expression-instrumentation path (`#[metered]` /
//! `#[measure]`, `Elapsed`, `HitCount`, ...) and the optional `recording`
//! module. The span-metrics path does not use them, which is why this module is
//! gated behind the `legacy` / `recording` features.

/// A metric that can measure an expression.
pub trait Measure {
    /// The owned recorder produced when entering this metric.
    type Recorder;

    /// Enter the metric, performing on-entry bookkeeping and returning a
    /// recorder that owns the handles needed to record the outcome later.
    fn enter(&self) -> Self::Recorder;
}

/// Records the outcome of a single measured execution.
pub trait Recorder<R> {
    /// Record a normally-completed execution. Called at most once per recorder.
    fn complete(&mut self, result: &R);
}

/// A recorder that records nothing after entry.
#[derive(Debug)]
pub struct NoOpRecorder;

impl<R> Recorder<R> for NoOpRecorder {
    fn complete(&mut self, _result: &R) {}
}

/// A one-shot flag for recorders that must act exactly once.
#[doc(hidden)]
#[derive(Debug)]
pub struct Armed(bool);

impl Armed {
    /// A newly-armed flag.
    #[doc(hidden)]
    pub fn new() -> Self {
        Armed(true)
    }

    /// Returns `true` exactly once (the first call), disarming thereafter.
    #[doc(hidden)]
    pub fn fire(&mut self) -> bool {
        core::mem::replace(&mut self.0, false)
    }
}

impl Default for Armed {
    fn default() -> Self {
        Armed::new()
    }
}

#[cfg(test)]
mod tests {
    use super::Armed;

    #[test]
    fn armed_fires_once() {
        let mut armed = Armed::new();
        assert!(armed.fire());
        assert!(!armed.fire());
        assert!(!armed.fire());
    }
}
