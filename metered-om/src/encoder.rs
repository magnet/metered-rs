//! The OpenMetrics text encoder and the low-level line writers.

use metered::bucket_histogram::HistogramSnapshot;
use metered::exponential_histogram::ExponentialSnapshot;
use metered::{
    HistogramData, HistogramValue, MetricExemplar, MetricFamilySchema, MetricSample, MetricSchema,
    MetricSink, MetricValues, SinkError, name_has_unit_suffix,
};
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display, Write};

/// How a histogram's buckets are rendered in the OpenMetrics text.
///
/// `#[non_exhaustive]`: further profiles may be added without a breaking change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum HistogramProfile {
    /// Classic Prometheus cumulative `le` buckets (the default).
    #[default]
    Le,
    /// VictoriaMetrics non-cumulative `vmrange` buckets. Exponential histograms
    /// render natively; classic bucket histograms fall back to `le` (they have
    /// no native `vmrange` form).
    VmRange,
}

/// Writes metrics in the OpenMetrics text exposition format.
///
/// Tracks which metric families have already been written, so a family split
/// across multiple label sets (e.g. an error breakdown) is declared exactly
/// once -- and so a family can never be resumed after another family's lines
/// were written between (OpenMetrics forbids interleaved families; see
/// [`OpenMetricsEncoder::encode_document`]). Call
/// [`OpenMetricsEncoder::finish`] to write the closing `# EOF`.
///
/// This is the OpenMetrics implementation of [`metered::MetricSink`]; a service
/// that wants a different exposition format implements its own sink and the
/// rest of `metered` is unchanged.
pub struct OpenMetricsEncoder<'a> {
    out: &'a mut (dyn Write + 'a),
    /// Every family whose lines have been written to `out`, declared or not.
    emitted: HashSet<String>,
    /// The family written last: the only one a later
    /// [`encode_document`](OpenMetricsEncoder::encode_document) call may
    /// continue without breaking the OpenMetrics contiguity rule.
    open: Option<String>,
    profile: HistogramProfile,
}

impl<'a> OpenMetricsEncoder<'a> {
    /// Creates an encoder writing into `out`, rendering histograms as classic
    /// `le` buckets.
    pub fn new(out: &'a mut (dyn Write + 'a)) -> Self {
        OpenMetricsEncoder {
            out,
            emitted: HashSet::new(),
            open: None,
            profile: HistogramProfile::Le,
        }
    }

    /// Sets the histogram bucket rendering profile (e.g.
    /// [`HistogramProfile::VmRange`] for VictoriaMetrics).
    pub fn histogram_profile(mut self, profile: HistogramProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Writes the closing `# EOF` line that terminates an OpenMetrics document.
    pub fn finish(self) -> Result<(), SinkError> {
        writeln!(self.out, "# EOF")?;
        Ok(())
    }

    /// Renders a schema and sampled values as OpenMetrics text.
    ///
    /// Family `# HELP` / `# TYPE` / `# UNIT` headers come from the schema (keyed
    /// to the exact family name, so metadata never leaks across composition
    /// boundaries); the samples come from the values. The document is emitted
    /// family by family -- each family's header immediately followed by all of
    /// its samples -- because OpenMetrics requires all lines of a MetricFamily
    /// to form one uninterrupted group (families must not be interleaved).
    ///
    /// Composing several trees onto one encoder is supported: a family shared
    /// across calls is declared once, and consecutive calls may continue the
    /// family written last. What the encoder refuses is *resuming* a family
    /// after another family's lines were written in between (A, B, A) -- that
    /// would silently violate the grouping MUST above, so it returns a
    /// [`SinkError`] instead of emitting an invalid document. Encode the trees
    /// that share a family back to back to stay contiguous. Call
    /// [`OpenMetricsEncoder::finish`] afterwards to write the terminal `# EOF`.
    ///
    /// Besides the non-contiguity error above, the only failure on this path
    /// is the writer's [`fmt::Error`], carried as the sink seam's [`SinkError`]
    /// through its allocation-free `From` conversion.
    pub fn encode_document(
        &mut self,
        schema: &MetricSchema,
        values: &MetricValues,
    ) -> Result<(), SinkError> {
        for group in group_families(schema, values) {
            // Within one call `group_families` guarantees contiguity, so only
            // a family already written by an *earlier* call can reappear here
            // -- legal only when it is still the open (last-written) family.
            if self.open.as_deref() != Some(group.name) {
                if self.emitted.contains(group.name) {
                    return Err(SinkError::from_source(format!(
                        "metric family `{}` reappeared after another family was \
                         written; OpenMetrics requires all lines of a family to \
                         form one contiguous group, so encode the trees that \
                         share a family back to back on the encoder",
                        group.name
                    )));
                }
                if let Some(family) = group.header {
                    write_family_header(&mut *self.out, family)?;
                }
                self.emitted.insert(group.name.to_owned());
                self.open = Some(group.name.to_owned());
            }
            for sample in group.samples {
                write_sample(&mut *self.out, sample)?;
            }
            let resolved = resolve_profile(self.profile, group.header);
            for histogram in group.histograms {
                for sample in histogram_samples(histogram, resolved) {
                    write_sample(&mut *self.out, &sample)?;
                }
            }
        }
        Ok(())
    }
}

/// The sample-name suffixes OpenMetrics defines for typed families, used to map
/// a sample back to the family it belongs to (`requests_total` -> `requests`).
const FAMILY_SUFFIXES: &[&str] = &[
    "_total", "_created", "_bucket", "_gcount", "_gsum", "_count", "_sum", "_info",
];

/// One family's contiguous slice of the document: its declared header (if the
/// schema knows it) plus every plain sample and structured histogram that
/// belongs to it.
pub(crate) struct FamilyGroup<'a> {
    /// The family's identity: the declared name, or the inferred base name for
    /// undeclared passthrough samples. Unique within one grouping.
    pub(crate) name: &'a str,
    pub(crate) header: Option<&'a MetricFamilySchema>,
    pub(crate) samples: Vec<&'a MetricSample>,
    pub(crate) histograms: Vec<&'a HistogramValue>,
}

/// Groups a document's samples by metric family, in schema order, so the text
/// renderers can satisfy the OpenMetrics grouping rule: *"All lines for a given
/// MetricFamily MUST be provided as one single group ... MetricFamilies MUST
/// NOT be interleaved."*
///
/// A sample joins a declared family by exact name match first (gauges,
/// statesets, summaries), then by stripping one of the spec's type suffixes
/// (`requests_total` -> `requests`). Samples with no declared family
/// (e.g. [`TextSourceTree`](crate::TextSourceTree) passthrough output) are
/// grouped by their inferred base name, after the declared families, in first
/// appearance order. Shared by [`OpenMetricsEncoder`] and the incremental
/// [`OpenMetricsRender`](crate::OpenMetricsRender) so both emit the exact same
/// document.
pub(crate) fn group_families<'a>(
    schema: &'a MetricSchema,
    values: &'a MetricValues,
) -> Vec<FamilyGroup<'a>> {
    let families = schema.families();
    let mut groups: Vec<FamilyGroup<'a>> = families
        .iter()
        .map(|family| FamilyGroup {
            name: family.name.as_str(),
            header: Some(family),
            samples: Vec::new(),
            histograms: Vec::new(),
        })
        .collect();
    let declared: HashMap<&str, usize> = families
        .iter()
        .enumerate()
        .map(|(index, family)| (family.name.as_str(), index))
        .collect();
    let mut undeclared: HashMap<&'a str, usize> = HashMap::new();

    let mut resolve = |groups: &mut Vec<FamilyGroup<'a>>, sample_name: &'a str| -> usize {
        if let Some(&index) = declared.get(sample_name) {
            return index;
        }
        let base = FAMILY_SUFFIXES
            .iter()
            .find_map(|suffix| sample_name.strip_suffix(suffix));
        if let Some(&index) = base.and_then(|base| declared.get(base)) {
            return index;
        }
        // Undeclared: group by the inferred base name so e.g. a passthrough
        // summary's `name` / `name_sum` / `name_count` stay contiguous.
        let key = base.unwrap_or(sample_name);
        *undeclared.entry(key).or_insert_with(|| {
            groups.push(FamilyGroup {
                name: key,
                header: None,
                samples: Vec::new(),
                histograms: Vec::new(),
            });
            groups.len() - 1
        })
    };

    for sample in values.samples() {
        let index = resolve(&mut groups, &sample.name);
        groups[index].samples.push(sample);
    }
    for histogram in values.histograms() {
        // A histogram's `name` is already the family name (no suffix).
        let index = resolve(&mut groups, &histogram.name);
        groups[index].histograms.push(histogram);
    }
    groups
}

/// Resolves a histogram family's effective render: the family's declared
/// intent ([`metered::HistogramRender`], carried on the
/// [`FamilyGroup::header`] the grouping already resolved -- `None` for an
/// undeclared passthrough family) gated by the document's capability
/// (`profile` — whether the scraper can ingest `vmrange` at all). Resolved
/// once per family group, not per series.
///
/// | declared \ document | `Le` (default)  | `VmRange`-capable |
/// |---------------------|-----------------|-------------------|
/// | `Auto`              | `le`            | `vmrange`         |
/// | `Le`                | `le`            | `le`              |
/// | `VmRange`           | `le` (degraded) | `vmrange`         |
/// | unknown (future)    | `le`            | `le` (fallback)   |
pub(crate) fn resolve_profile(
    document: HistogramProfile,
    family: Option<&MetricFamilySchema>,
) -> HistogramProfile {
    match document {
        // The scraper cannot ingest vmrange: every declaration degrades to le.
        HistogramProfile::Le => HistogramProfile::Le,
        HistogramProfile::VmRange => match family
            .map(|family| family.histogram_render)
            .unwrap_or_default()
        {
            metered::HistogramRender::Le => HistogramProfile::Le,
            // The document is vmrange-capable, so an explicit or defaulted
            // declaration renders the native form.
            metered::HistogramRender::Auto | metered::HistogramRender::VmRange => {
                HistogramProfile::VmRange
            }
            // `HistogramRender` is `#[non_exhaustive]`: a declaration this
            // encoder does not know falls back to classic `le` -- the one
            // rendering every scraper ingests -- rather than guessing that a
            // future declaration wants `vmrange`.
            _ => HistogramProfile::Le,
        },
    }
}

/// Expands a structured histogram into the bucket / `_sum` / `_count` samples
/// for the given profile. Shared by [`OpenMetricsEncoder`] and the incremental
/// [`OpenMetricsRender`](crate::OpenMetricsRender).
pub(crate) fn histogram_samples(
    histogram: &HistogramValue,
    profile: HistogramProfile,
) -> Vec<MetricSample> {
    match (&histogram.data, profile) {
        (HistogramData::Exponential(snapshot), HistogramProfile::VmRange) => {
            vmrange_samples(&histogram.name, &histogram.labels, snapshot)
        }
        (HistogramData::Exponential(snapshot), _) => le_samples(
            &histogram.name,
            &histogram.labels,
            &snapshot.to_histogram_snapshot(),
        ),
        (HistogramData::Classic(snapshot), _) => {
            le_samples(&histogram.name, &histogram.labels, snapshot)
        }
        // A recorded form this encoder does not know (`HistogramData` is
        // `#[non_exhaustive]`): skip the family's samples rather than guess a
        // rendering; the scrape itself must still succeed.
        _ => Vec::new(),
    }
}

fn le_samples(
    name: &str,
    labels: &[(String, String)],
    snapshot: &HistogramSnapshot,
) -> Vec<MetricSample> {
    let bucket_name = format!("{name}_bucket");
    let mut samples = Vec::with_capacity(snapshot.buckets.len() + 2);
    for bucket in &snapshot.buckets {
        let le = if bucket.le.is_infinite() {
            "+Inf".to_owned()
        } else {
            bucket.le.to_string()
        };
        let mut bucket_labels = labels.to_vec();
        bucket_labels.push(("le".to_owned(), le));
        let exemplar = bucket.exemplar.as_ref().map(|exemplar| {
            MetricExemplar::new(
                exemplar.labels.clone(),
                exemplar.value,
                exemplar.timestamp_seconds,
            )
        });
        samples.push(MetricSample::new(
            bucket_name.clone(),
            bucket_labels,
            bucket.cumulative_count,
            exemplar,
        ));
    }
    push_sum_count(&mut samples, name, labels, snapshot.sum, snapshot.count);
    samples
}

fn vmrange_samples(
    name: &str,
    labels: &[(String, String)],
    snapshot: &ExponentialSnapshot,
) -> Vec<MetricSample> {
    let bucket_name = format!("{name}_bucket");
    let mut samples = Vec::with_capacity(snapshot.buckets.len() + 4);
    let push_bucket = |samples: &mut Vec<MetricSample>,
                       vmrange: String,
                       count: u64,
                       exemplar: Option<MetricExemplar>| {
        let mut bucket_labels = labels.to_vec();
        bucket_labels.push(("vmrange".to_owned(), vmrange));
        samples.push(MetricSample::new(
            bucket_name.clone(),
            bucket_labels,
            count,
            exemplar,
        ));
    };

    // Every observation must land in exactly one bucket so the per-bucket
    // counts sum to `_count` and VictoriaMetrics' `histogram_quantile` (which
    // works off the buckets alone) sees the full distribution. Non-positive
    // observations are recorded at 0 -- VictoriaMetrics' own exposition uses a
    // `0...`-lower range for below-range values -- with a constant range label
    // so no time-series churn.
    if snapshot.zero_count > 0 {
        push_bucket(&mut samples, "0...0".to_owned(), snapshot.zero_count, None);
    }
    for bucket in &snapshot.buckets {
        // VictoriaMetrics reads the `lo...hi` edges from the label; the counts
        // are per-bucket (not cumulative). Rust's `{:.3e}` writes the exponent
        // unpadded (`4.084e-3` where VictoriaMetrics itself writes
        // `4.084e-03`); both parse to the same boundary value.
        let vmrange = format!("{:.3e}...{:.3e}", bucket.lower, bucket.upper);
        // Exemplar parity with the `le` render: the sampled per-bucket
        // exemplar rides the bucket sample in either encoding.
        let exemplar = bucket.exemplar.as_ref().map(|exemplar| {
            MetricExemplar::new(
                exemplar.labels.clone(),
                exemplar.value,
                exemplar.timestamp_seconds,
            )
        });
        push_bucket(&mut samples, vmrange, bucket.count, exemplar);
    }
    // Observations above the representable range: an open `...+Inf` tail (the
    // VictoriaMetrics convention for above-range values). The snapshot does
    // not carry the histogram's configured maximum, so the boundary is the
    // highest populated bucket's upper edge -- every overflow observation is
    // above it, so the containment is correct even if the edge is looser than
    // the true configured bound.
    if snapshot.overflow_count > 0 {
        let lower = snapshot.buckets.last().map_or(0.0, |bucket| bucket.upper);
        push_bucket(
            &mut samples,
            format!("{lower:.3e}...+Inf"),
            snapshot.overflow_count,
            None,
        );
    }
    push_sum_count(&mut samples, name, labels, snapshot.sum, snapshot.count);
    samples
}

fn push_sum_count(
    samples: &mut Vec<MetricSample>,
    name: &str,
    labels: &[(String, String)],
    sum: f64,
    count: u64,
) {
    samples.push(MetricSample::new(
        format!("{name}_sum"),
        labels.to_vec(),
        sum,
        None,
    ));
    samples.push(MetricSample::new(
        format!("{name}_count"),
        labels.to_vec(),
        count,
        None,
    ));
}

impl MetricSink for OpenMetricsEncoder<'_> {
    fn encode_document(
        &mut self,
        schema: &MetricSchema,
        values: &MetricValues,
    ) -> Result<(), SinkError> {
        OpenMetricsEncoder::encode_document(self, schema, values)
    }
}

/// Writes a family's `# HELP` / `# TYPE` / `# UNIT` header lines. The single
/// header writer shared by [`OpenMetricsEncoder::encode_document`] and the
/// incremental [`OpenMetricsRender`](crate::OpenMetricsRender).
pub(crate) fn write_family_header(out: &mut dyn Write, family: &MetricFamilySchema) -> fmt::Result {
    if let Some(help) = &family.help {
        writeln!(out, "# HELP {} {}", family.name, escape_help(help.as_str()))?;
    }
    writeln!(
        out,
        "# TYPE {} {}",
        family.name,
        family.metric_type.as_str()
    )?;
    if let Some(unit) = &family.unit {
        // OpenMetrics 1.0 requires a declared unit to be a suffix of the family
        // name -- either the whole name (`seconds` + `seconds`) or an
        // `_`-separated tail (`foo_seconds` + `seconds`). Prometheus's
        // OpenMetrics parser (the `application/openmetrics-text; version=1.0.0`
        // reader) rejects the ENTIRE scrape when this is violated, so one
        // mislabeled unit would drop every metric in the document. Emit the
        // `# UNIT` line only when the final, prefixed name conforms; otherwise
        // drop just that metadata line and keep the family renderable. We never
        // panic on this path: a naming mistake must not abort the exposition
        // (same reason `Gauge::decr` saturates instead of asserting). The rule
        // itself is core's `name_has_unit_suffix` -- the same check
        // `MetricSchema::validate` applies, so the validator and this encoder
        // can never disagree on which units are emittable.
        if name_has_unit_suffix(&family.name, unit.as_str()) {
            writeln!(out, "# UNIT {} {}", family.name, unit.as_str())?;
        } else {
            #[cfg(debug_assertions)]
            eprintln!(
                "metered-om: dropping non-conformant `# UNIT {name} {unit}` \
                 (OpenMetrics requires the unit to be the family name or an \
                 `_`-separated suffix of it; rename the family to `{name}_{unit}` \
                 to emit it and keep Prometheus from rejecting the scrape)",
                name = family.name,
                unit = unit.as_str(),
            );
        }
    }
    Ok(())
}

/// OpenMetrics 1.0: "The combined length of the label names and values of an
/// Exemplar's LabelSet MUST NOT exceed 128 UTF-8 characters."
const EXEMPLAR_LABELSET_MAX_CHARS: usize = 128;

/// Whether an exemplar's labelset fits the OpenMetrics 128-character limit
/// (counted in UTF-8 characters over all label names and values, per the spec).
fn exemplar_labels_fit(labels: &[(String, String)]) -> bool {
    let chars: usize = labels
        .iter()
        .map(|(name, value)| name.chars().count() + value.chars().count())
        .sum();
    chars <= EXEMPLAR_LABELSET_MAX_CHARS
}

/// Writes one sample line, including an optional trailing exemplar. The single
/// sample writer shared by the eager and incremental renderers.
///
/// An exemplar whose labelset exceeds the OpenMetrics 128-character limit is
/// dropped (the sample itself still renders): emitting it would make a
/// conformant scraper such as Prometheus reject the whole exposition, and
/// truncating labels would fabricate values (e.g. a trace id that no longer
/// resolves), so omission is the conformant choice.
pub(crate) fn write_sample(out: &mut dyn Write, sample: &MetricSample) -> fmt::Result {
    write!(
        out,
        "{}{} {}",
        sample.name,
        format_owned_labels(&sample.labels),
        sample.value
    )?;
    if let Some(exemplar) = &sample.exemplar {
        if exemplar_labels_fit(&exemplar.labels) {
            // The exemplar grammar requires the labelset braces even when the
            // labelset is empty (`# {} value`), unlike a sample's label block.
            if exemplar.labels.is_empty() {
                write!(out, " # {{}} {}", exemplar.value)?;
            } else {
                write!(
                    out,
                    " # {} {}",
                    format_owned_labels(&exemplar.labels),
                    exemplar.value
                )?;
            }
            if let Some(timestamp) = exemplar.timestamp {
                write!(out, " {timestamp}")?;
            }
        }
    }
    writeln!(out)
}

/// Formats a Prometheus label block, e.g. `{le="0.005",service="x"}`, or the
/// empty string when there are no labels.
fn format_labels(labels: &[(&str, &str)]) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let mut out = String::from("{");
    for (i, (k, v)) in labels.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{k}=\"{}\"", EscapedLabel(v));
    }
    out.push('}');
    out
}

fn format_owned_labels(labels: &[(String, String)]) -> String {
    let borrowed: Vec<(&str, &str)> = labels
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    format_labels(&borrowed)
}

/// Escapes `HELP` text per OpenMetrics: backslash and newline only.
fn escape_help(help: &str) -> String {
    let mut out = String::with_capacity(help.len());
    for ch in help.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

struct EscapedLabel<'a>(&'a str);

impl Display for EscapedLabel<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for ch in self.0.chars() {
            match ch {
                '\\' => f.write_str("\\\\")?,
                '"' => f.write_str("\\\"")?,
                '\n' => f.write_str("\\n")?,
                other => f.write_char(other)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
