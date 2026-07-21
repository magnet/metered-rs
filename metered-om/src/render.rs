//! Incremental, budget-driven OpenMetrics rendering.
//!
//! [`OpenMetricsEncoder::encode_document`](crate::OpenMetricsEncoder::encode_document)
//! renders a whole document in one call. For very large metric sets that is a
//! lot of synchronous work in one go. [`OpenMetricsRender`] renders the same
//! document incrementally: each [`step`](OpenMetricsRender::step) writes at most
//! `budget` items (family declarations or samples) and reports whether more work
//! remains. This is deliberately *not* async -- you can drive it from anywhere,
//! yielding between steps however you like.
//!
//! ```
//! use metered::{Counter, Registry};
//! use metered_om::{OpenMetricsRender, RenderProgress};
//! use std::sync::atomic::AtomicU64;
//!
//! let requests = AtomicU64::new(0);
//! requests.incr();
//! let mut registry = Registry::with_prefix("app");
//! registry.register(metered::entry::metric("requests").source(&requests).help("Requests"));
//!
//! let schema = registry.schema();
//! let values = registry.values();
//!
//! // Drive it by hand, a couple of items at a time.
//! let mut render = OpenMetricsRender::new(&schema, &values);
//! let mut out = String::new();
//! while render.step(&mut out, 2).unwrap() == RenderProgress::Pending {}
//!
//! assert!(out.contains("# TYPE app_requests counter"));
//! assert!(out.contains("app_requests_total 1"));
//! assert!(out.trim_end().ends_with("# EOF"));
//! ```
//!
//! For async callers, [`RenderFuture`] wraps the same stepper in a dependency-free
//! [`Future`](core::future::Future) that yields back to the executor after each
//! budget chunk.

use crate::encoder::{histogram_samples, write_family_header, write_sample, HistogramProfile};
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use metered::{MetricSchema, MetricValues};
use std::fmt::{self, Write};

/// Whether an [`OpenMetricsRender::step`] left work for a later call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderProgress {
    /// More items remain; call [`OpenMetricsRender::step`] again.
    Pending,
    /// The document (including the closing `# EOF`) is fully written.
    Done,
}

#[derive(Clone, Debug)]
enum Cursor {
    Families(usize),
    Samples(usize),
    Histograms {
        /// The next histogram to expand.
        index: usize,
        /// The current histogram's expanded samples, emitted one budget item
        /// at a time so a large histogram cannot blow through the budget.
        pending: Vec<metered::MetricSample>,
        /// The next pending sample to write.
        next: usize,
    },
    Eof,
    Done,
}

/// An incremental renderer over a borrowed schema and value set.
///
/// Renders the exact same document as
/// [`OpenMetricsEncoder::encode_document`](crate::OpenMetricsEncoder::encode_document)
/// followed by [`finish`](crate::OpenMetricsEncoder::finish), but in
/// budget-bounded steps. The same output sink must be passed to every
/// [`step`](OpenMetricsRender::step) so the document accumulates in order.
pub struct OpenMetricsRender<'a> {
    schema: &'a MetricSchema,
    values: &'a MetricValues,
    cursor: Cursor,
    profile: HistogramProfile,
}

impl<'a> OpenMetricsRender<'a> {
    /// Creates a renderer for `schema` and `values` (classic `le` histograms).
    pub fn new(schema: &'a MetricSchema, values: &'a MetricValues) -> Self {
        OpenMetricsRender::with_profile(schema, values, HistogramProfile::Le)
    }

    /// Creates a renderer with an explicit histogram [`HistogramProfile`].
    pub fn with_profile(
        schema: &'a MetricSchema,
        values: &'a MetricValues,
        profile: HistogramProfile,
    ) -> Self {
        OpenMetricsRender {
            schema,
            values,
            cursor: Cursor::Families(0),
            profile,
        }
    }

    /// Returns `true` once the document is fully written.
    pub fn is_done(&self) -> bool {
        matches!(self.cursor, Cursor::Done)
    }

    /// Writes at most `budget` items into `out`.
    ///
    /// An item is one family declaration (its `# HELP` / `# TYPE` / `# UNIT`
    /// block) or one sample line -- including each bucket line of a histogram,
    /// whose position is part of the cursor, so a large histogram is emitted
    /// across steps rather than blowing through the budget in one go. A
    /// `budget` of `0` is treated as `1` so a step always makes progress.
    /// Returns [`RenderProgress::Done`] once the closing `# EOF` has been
    /// written.
    pub fn step(
        &mut self,
        out: &mut dyn Write,
        budget: usize,
    ) -> Result<RenderProgress, fmt::Error> {
        let budget = budget.max(1);
        let mut used = 0;
        while used < budget {
            match &mut self.cursor {
                Cursor::Families(index) => match self.schema.families().get(*index) {
                    Some(family) => {
                        write_family_header(out, family)?;
                        *index += 1;
                        used += 1;
                    }
                    None => self.cursor = Cursor::Samples(0),
                },
                Cursor::Samples(index) => match self.values.samples().get(*index) {
                    Some(sample) => {
                        write_sample(out, sample)?;
                        *index += 1;
                        used += 1;
                    }
                    None => {
                        self.cursor = Cursor::Histograms {
                            index: 0,
                            pending: Vec::new(),
                            next: 0,
                        }
                    }
                },
                Cursor::Histograms {
                    index,
                    pending,
                    next,
                } => {
                    if let Some(sample) = pending.get(*next) {
                        write_sample(out, sample)?;
                        *next += 1;
                        used += 1;
                    } else if let Some(histogram) = self.values.histograms().get(*index) {
                        let resolved = crate::encoder::resolve_profile(
                            self.profile,
                            self.schema,
                            &histogram.name,
                        );
                        *pending = histogram_samples(histogram, resolved);
                        *next = 0;
                        *index += 1;
                    } else {
                        self.cursor = Cursor::Eof;
                    }
                }
                Cursor::Eof => {
                    writeln!(out, "# EOF")?;
                    self.cursor = Cursor::Done;
                    return Ok(RenderProgress::Done);
                }
                Cursor::Done => return Ok(RenderProgress::Done),
            }
        }

        Ok(if self.is_done() {
            RenderProgress::Done
        } else {
            RenderProgress::Pending
        })
    }

    /// Drives the renderer to completion and returns the full document.
    pub fn render_to_string(mut self) -> Result<String, fmt::Error> {
        let mut out = String::new();
        while self.step(&mut out, usize::MAX)? == RenderProgress::Pending {}
        Ok(out)
    }
}

/// A dependency-free [`Future`] that renders a document in budget chunks.
///
/// Each `poll` writes one `budget`-sized chunk; if more work remains it wakes
/// itself and returns [`Poll::Pending`], giving the executor a chance to run
/// other tasks before the next chunk. On completion it yields the full document.
///
/// ```
/// use metered::{Counter, Registry};
/// use metered_om::RenderFuture;
/// use std::sync::atomic::AtomicU64;
///
/// # async fn run() {
/// let requests = AtomicU64::new(0);
/// requests.incr();
/// let mut registry = Registry::new();
/// registry.register(metered::entry::metric("requests").source(&requests).help("Requests"));
/// let schema = registry.schema();
/// let values = registry.values();
///
/// let text = RenderFuture::new(&schema, &values, 64).await.unwrap();
/// assert!(text.trim_end().ends_with("# EOF"));
/// # }
/// ```
pub struct RenderFuture<'a> {
    render: OpenMetricsRender<'a>,
    out: String,
    budget: usize,
}

impl<'a> RenderFuture<'a> {
    /// Creates a future rendering `schema`/`values`, writing up to `budget`
    /// items between executor yields.
    pub fn new(schema: &'a MetricSchema, values: &'a MetricValues, budget: usize) -> Self {
        RenderFuture {
            render: OpenMetricsRender::new(schema, values),
            out: String::new(),
            budget,
        }
    }
}

impl Future for RenderFuture<'_> {
    type Output = Result<String, fmt::Error>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match this.render.step(&mut this.out, this.budget) {
            Err(error) => Poll::Ready(Err(error)),
            Ok(RenderProgress::Done) => Poll::Ready(Ok(std::mem::take(&mut this.out))),
            Ok(RenderProgress::Pending) => {
                // Re-schedule immediately: we made progress and want to keep
                // going, but yield first so other tasks can run.
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoder::OpenMetricsEncoder;
    use metered::{Metric, MetricSchema, MetricValues};
    use std::sync::atomic::{AtomicI64, AtomicU64};

    fn schema_and_values() -> (MetricSchema, MetricValues) {
        let requests = AtomicU64::new(0);
        metered::Counter::incr_by(&requests, 3);
        let depth = AtomicI64::new(0);
        metered::Gauge::set(&depth, 7);

        let mut schema = MetricSchema::new();
        schema.set_help_for("requests", "Requests handled");
        requests.describe_metric("requests", &[("svc", "api")], &mut schema);
        schema.set_metadata_for(
            "queue_depth",
            Some(metered::Help::from("Queue depth")),
            Some(metered::Unit::Items),
        );
        depth.describe_metric("queue_depth", &[("svc", "api")], &mut schema);

        let mut values = MetricValues::new();
        requests.collect_metric("requests", &[("svc", "api")], &mut values);
        depth.collect_metric("queue_depth", &[("svc", "api")], &mut values);

        (schema, values)
    }

    fn reference_document(schema: &MetricSchema, values: &MetricValues) -> String {
        let mut buf = String::new();
        {
            let mut encoder = OpenMetricsEncoder::new(&mut buf);
            encoder.encode_document(schema, values).unwrap();
            encoder.finish().unwrap();
        }
        buf
    }

    #[test]
    fn small_budget_matches_one_shot_encode_document() {
        let (schema, values) = schema_and_values();
        let expected = reference_document(&schema, &values);

        let mut render = OpenMetricsRender::new(&schema, &values);
        let mut out = String::new();
        let mut steps = 0;
        loop {
            steps += 1;
            if render.step(&mut out, 1).unwrap() == RenderProgress::Done {
                break;
            }
        }

        assert_eq!(out, expected);
        // Two families (each one item) + two samples + EOF => more than one step.
        assert!(steps > 1, "a budget of 1 should take several steps");
    }

    #[test]
    fn large_budget_completes_in_a_single_step() {
        let (schema, values) = schema_and_values();
        let expected = reference_document(&schema, &values);

        let mut render = OpenMetricsRender::new(&schema, &values);
        let mut out = String::new();
        assert_eq!(
            render.step(&mut out, usize::MAX).unwrap(),
            RenderProgress::Done
        );
        assert_eq!(out, expected);
        assert!(render.is_done());
    }

    #[test]
    fn zero_budget_still_makes_progress() {
        let (schema, values) = schema_and_values();
        let mut render = OpenMetricsRender::new(&schema, &values);
        let mut out = String::new();
        // A zero budget is clamped to one item, so this terminates.
        while render.step(&mut out, 0).unwrap() == RenderProgress::Pending {}
        assert_eq!(out, reference_document(&schema, &values));
    }

    #[test]
    fn histogram_buckets_respect_the_step_budget() {
        use metered::bucket_histogram::{BucketHistogram, Buckets};

        // A 14-bound histogram expands to 17 sample lines; with a budget of 2
        // no single step may emit more than 2 lines, and the accumulated
        // document must still match the one-shot encoder exactly.
        let histogram = BucketHistogram::new(Buckets::seconds_default());
        histogram.observe(0.003);
        histogram.observe(4.2);

        let mut schema = MetricSchema::new();
        histogram.describe_metric("latency_seconds", &[], &mut schema);
        let mut values = MetricValues::new();
        histogram.collect_metric("latency_seconds", &[], &mut values);
        let expected = reference_document(&schema, &values);

        let mut render = OpenMetricsRender::new(&schema, &values);
        let mut out = String::new();
        let mut previous_lines = 0;
        loop {
            let progress = render.step(&mut out, 2).unwrap();
            let lines = out.lines().count();
            assert!(
                lines - previous_lines <= 2,
                "a step wrote {} lines, past its budget of 2",
                lines - previous_lines
            );
            previous_lines = lines;
            if progress == RenderProgress::Done {
                break;
            }
        }
        assert_eq!(out, expected);
    }

    #[test]
    fn render_to_string_matches_reference() {
        let (schema, values) = schema_and_values();
        let expected = reference_document(&schema, &values);
        let rendered = OpenMetricsRender::new(&schema, &values)
            .render_to_string()
            .unwrap();
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_future_yields_then_completes() {
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::Arc;
        use std::task::{Context, Poll, Wake, Waker};

        struct CountingWaker(std::sync::atomic::AtomicUsize);
        impl Wake for CountingWaker {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            fn wake_by_ref(self: &Arc<Self>) {
                self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }

        let (schema, values) = schema_and_values();
        let expected = reference_document(&schema, &values);

        let counting = Arc::new(CountingWaker(std::sync::atomic::AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&counting));
        let mut cx = Context::from_waker(&waker);

        // Budget of one item forces several Pending polls before completion.
        let mut future = RenderFuture::new(&schema, &values, 1);
        let result = loop {
            match Pin::new(&mut future).poll(&mut cx) {
                Poll::Pending => continue,
                Poll::Ready(result) => break result.unwrap(),
            }
        };

        assert_eq!(result, expected);
        assert!(
            counting.0.load(std::sync::atomic::Ordering::Relaxed) > 0,
            "a small budget should have yielded to the executor at least once"
        );
    }
}
