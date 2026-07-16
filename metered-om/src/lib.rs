//! OpenMetrics text exposition for [`metered`].
//!
//! `metered` is the instrumentation core: it describes a [`metered::MetricSchema`] and
//! collects [`metered::MetricValues`], independent of any wire format. This crate is one
//! *sink* over that model -- the OpenMetrics text exposition Prometheus and
//! VictoriaMetrics scrape -- selected by a service at the exposition site. The
//! core [`MetricSink`](metered::MetricSink) trait is the seam, so a future
//! Prometheus-protobuf sink can live in its own crate without touching either
//! `metered` or this one.
//!
//! ```
//! use metered::{Counter, MetricTree};
//! use metered_om::{OpenMetricsEncoder, OpenMetricsExt};
//! use std::sync::atomic::AtomicU64;
//!
//! let hits = AtomicU64::new(0);
//! hits.incr();
//!
//! // Explicit sink: the service owns the encoder.
//! let mut buf = String::new();
//! {
//!     let mut enc = OpenMetricsEncoder::new(&mut buf);
//!     hits.encode("requests", &[("service", "demo")], &mut enc).unwrap();
//!     enc.finish().unwrap();
//! }
//! assert!(buf.contains("requests_total{service=\"demo\"} 1"));
//!
//! // Or the `encode_to_string` convenience for a whole tree.
//! let text = hits.encode_to_string().unwrap();
//! assert!(text.trim_end().ends_with("# EOF"));
//! ```
//!
//! Parse emitted text back into a structural model with
//! [`OpenMetricsDocument::parse`] when tests or tooling need to inspect the
//! exposition without string matching.

#![deny(missing_docs)]

mod encoder;
mod ext;
#[cfg(feature = "hyper-1")]
pub mod hyper_1;
mod lex;
pub mod prom_text;
mod render;
mod snapshot;
mod text;

pub use encoder::{HistogramProfile, OpenMetricsEncoder};
pub use ext::{OpenMetricsExt, OpenMetricsRegistryExt, OpenMetricsViewExt};
pub use prom_text::TextSourceTree;
pub use render::{OpenMetricsRender, RenderFuture, RenderProgress};
pub use snapshot::{
    MetricTreeSnapshot, RegistrySnapshot, Snapshot, SnapshotCache, SnapshotConfig, SnapshotTarget,
};
pub use text::{
    OpenMetricsDocument, OpenMetricsExemplar, OpenMetricsFamily, OpenMetricsSample,
    ParseOpenMetricsError,
};
