//! `encode_to_string` conveniences layered over the core `encode` methods.
//!
//! The core crate keeps the sink-generic `encode(.., &mut dyn MetricSink)`
//! methods; these extension traits add the OpenMetrics-specific "render the
//! whole thing to a `String` (with `# EOF`)" shortcut without baking the text
//! format into core. Bring the relevant trait into scope to use the method.

use crate::encoder::OpenMetricsEncoder;
use metered::{MetricTree, MetricTreeView, Registry, SinkError};

fn to_string(
    write: impl FnOnce(&mut OpenMetricsEncoder<'_>) -> Result<(), SinkError>,
) -> Result<String, SinkError> {
    let mut buf = String::new();
    {
        let mut encoder = OpenMetricsEncoder::new(&mut buf);
        write(&mut encoder)?;
        encoder.finish()?;
    }
    Ok(buf)
}

/// Adds [`encode_to_string`](OpenMetricsExt::encode_to_string) to any
/// [`MetricTree`] (leaf metrics, `#[derive(MetricTree)]` roots, `Family`,
/// `Renamed`/`Flatten`, ...).
pub trait OpenMetricsExt {
    /// Renders this tree to a complete OpenMetrics document, including the
    /// closing `# EOF`.
    ///
    /// The only failure is the text writer's, carried as the sink seam's
    /// [`SinkError`].
    fn encode_to_string(&self) -> Result<String, SinkError>;
}

impl<T: MetricTree + ?Sized> OpenMetricsExt for T {
    fn encode_to_string(&self) -> Result<String, SinkError> {
        to_string(|encoder| self.encode("", &[], encoder))
    }
}

/// Adds [`encode_to_string`](OpenMetricsRegistryExt::encode_to_string) to a
/// [`Registry`]. (`Registry` is not itself a [`MetricTree`], so it needs its
/// own extension.)
pub trait OpenMetricsRegistryExt {
    /// Encodes the whole registry to an OpenMetrics document string, including
    /// the closing `# EOF`.
    fn encode_to_string(&self) -> Result<String, SinkError>;
}

impl OpenMetricsRegistryExt for Registry<'_> {
    fn encode_to_string(&self) -> Result<String, SinkError> {
        to_string(|encoder| self.encode(encoder))
    }
}

/// Adds [`encode_to_string`](OpenMetricsViewExt::encode_to_string) to a
/// [`MetricTreeView`], which encodes against a borrowed context.
pub trait OpenMetricsViewExt<C> {
    /// Encodes the selected metric tree to an OpenMetrics document string,
    /// including the closing `# EOF`.
    fn encode_to_string(&self, context: &C) -> Result<String, SinkError>;
}

impl<C> OpenMetricsViewExt<C> for MetricTreeView<'_, C> {
    fn encode_to_string(&self, context: &C) -> Result<String, SinkError> {
        to_string(|encoder| self.encode(context, encoder))
    }
}
