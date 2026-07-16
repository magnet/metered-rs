//! Core metric value traits and small concrete helpers.
//!
//! The scalar traits are split along the read/write seam: [`CounterSource`]
//! and [`GaugeSource`] are the read-only sides exposition samples from, while
//! [`Counter`] and [`Gauge`] extend them with mutation for values the metric
//! owner may actually move. The standard atomics (`AtomicU64`, `AtomicI64`)
//! and metered's semantic wrappers (`HitCount`, `ErrorCount`, `NoneCount`,
//! `InFlight`) implement the full instrument traits; read-only adapters over
//! application state ([`crate::adapter`]) implement only the source side, so
//! they can never be handed out where mutation is expected. A service with its
//! own counter or gauge type implements the narrow trait it can honor, while
//! metered owns the exposition kind through the typed registry entries or the
//! built-in [`Metric`](crate::Metric) impls.
//!
//! [`Info`] is the corresponding value trait for OpenMetrics info metrics. The
//! concrete [`InfoMetric`] stores runtime-mutable [`Labels`].

mod counter;
mod gauge;
mod info;
mod stateset;

pub use counter::{AsCounter, Counter, CounterSource};
pub use gauge::{AsGauge, Gauge, GaugeSource};
pub use info::{Info, InfoMetric, Labels};
pub use stateset::StateSet;
