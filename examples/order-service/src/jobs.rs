//! A background job runner (the order outbox).
//!
//! It owns a real queue and processes one job per `run_once`. Its run count and
//! per-outcome breakdown come from the `jobs.run` span via a [`SpanMetric`] the
//! runner owns and mounts under `jobs_run`; only the queue depth is exported
//! directly, as a gauge computed from the live queue length.

use metered::{MetricTreeView, MetricsView};
use metered_tracing::{SpanMetric, SpanMetricsSource, SpanRecorder};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;

pub struct JobRunner {
    run: SpanMetric,
    queue: Mutex<VecDeque<Job>>,
}

struct Job;

/// The runner owns the span metrics for the `jobs.run` spans it emits.
impl SpanMetricsSource for JobRunner {
    fn span_recorders(self: &Arc<Self>) -> Vec<Box<dyn SpanRecorder>> {
        vec![Box::new(self.run.clone())]
    }
}

impl JobRunner {
    pub fn demo() -> Self {
        let mut queue = VecDeque::new();
        queue.push_back(Job);
        JobRunner {
            run: SpanMetric::for_span("jobs.run")
                .help("Background job runs")
                // semconv span attribute `job.outcome` -> OpenMetrics label `outcome`.
                .label("outcome", "job.outcome")
                .build(),
            queue: Mutex::new(queue),
        }
    }

    #[tracing::instrument(name = "jobs.run", skip_all, fields(job.outcome = tracing::field::Empty))]
    pub fn run_once(&self) {
        let job = self.queue.lock().pop_front();
        let outcome = match job {
            Some(_job) => "ran",
            None => "empty",
        };
        tracing::Span::current().record("job.outcome", outcome);
    }

    fn queue_depth(&self) -> usize {
        self.queue.lock().len()
    }
}

impl MetricsView for JobRunner {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        let mut view = MetricTreeView::new();
        // Span-derived run metric, mounted under `run` (so `..._jobs_run_*`).
        view.register(metered::entry::metric("run").select(|jobs: &JobRunner| &jobs.run));
        view.register(
            metered::entry::gauge_value("queue_depth")
                .read(|jobs: &JobRunner| jobs.queue_depth())
                .help("Background jobs waiting in the queue"),
        );
        view
    }
}
