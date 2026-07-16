//! The runtime metric schema: a registry's metric contract.
//!
//! A schema is about *shape*, not values: family names, types, HELP text,
//! units, and label names. It is the foundation other concerns build on --
//! PromQL/MetricsQL [`query`](crate::query) generation and the
//! [`values`](crate::values) model are downstream of it -- so it can feed
//! documentation, dashboard templates, and review tools without scraping a live
//! `/metrics` endpoint.

use crate::labels::slices::with_labels;
use crate::meta::{Help, Unit};
use crate::metric_tree::MetricType;
use std::collections::HashMap;
use std::fmt;

/// A schema-integrity problem surfaced by [`MetricSchema::validate`].
///
/// Building a schema never fails -- a scrape must always render -- so problems
/// found during construction are recorded and reported through the fallible
/// [`validate`](MetricSchema::validate) check a service runs once at startup
/// (or in a test), rather than aborting a scrape.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchemaError {
    /// One family name was declared with two different metric types. The type
    /// declared first is kept for rendering; the conflicting one is reported.
    TypeConflict {
        /// The family name declared under conflicting types.
        name: String,
        /// The type kept (declared first).
        kept: MetricType,
        /// The conflicting type that was rejected.
        rejected: MetricType,
    },
    /// A family carries a `# UNIT` whose token is not a suffix of the family
    /// name. OpenMetrics requires the unit to suffix the name (a `seconds`
    /// unit needs a `_seconds` name), or a consumer cannot recover the unit
    /// from the series name.
    UnitSuffix {
        /// The family name.
        name: String,
        /// The declared unit token.
        unit: String,
    },
    /// One family declaration carried the same label name more than once. The
    /// name is deduplicated for rendering, but a declaration that repeats a
    /// label name means two values were composed for one name -- had it
    /// reached the wire it would emit duplicate label names in one series,
    /// which OpenMetrics forbids. The internal composition paths resolve
    /// collisions deterministically (the inner pair wins), so this reports a
    /// duplicated declaration handed to
    /// [`add_family`](MetricSchema::add_family) directly.
    DuplicateLabel {
        /// The family name declared with a repeated label name.
        name: String,
        /// The label name that was declared more than once.
        label: String,
    },
    /// A family declared a label name the OpenMetrics encoder owns as a
    /// sample-structure label: `le`, `quantile`, or `vmrange`. The encoder
    /// attaches these names itself when it renders bucket bounds and
    /// quantiles, so a user-declared pair would collide with (or be shadowed
    /// by) the structural one on the wire. The declaration is kept for
    /// rendering; the conflict is reported here.
    ///
    /// The metric type that structurally owns the name is exempt on the
    /// public [`add_family`](MetricSchema::add_family) path: a gauge
    /// histogram declares its own `le` dimension and a summary its own
    /// `quantile`, because their samples carry the pair directly. Instruments
    /// that compose that structural pair must go through
    /// `add_family_structural`, which
    /// checks the *incoming* (user/inherited) labels for a collision with the
    /// owned name before compose shadows the evidence.
    ReservedLabel {
        /// The family name declared with a reserved label.
        name: String,
        /// The reserved label name (`le`, `quantile`, or `vmrange`).
        label: String,
    },
    /// An entry could not describe its schema in a context-free walk (a
    /// dynamic [`family_view`](crate::MetricTreeView::family_view) group): it
    /// resolves its
    /// metrics through a runtime projection, so its families are missing from
    /// the schema while its values still render. Declare the element's
    /// metrics through typed entries or a directly-held tree instead.
    UndescribedEntry {
        /// The full metric name prefix of the undescribed entry.
        name: String,
    },
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::TypeConflict {
                name,
                kept,
                rejected,
            } => write!(
                f,
                "family `{name}` declared as both `{}` and `{}`",
                kept.as_str(),
                rejected.as_str()
            ),
            SchemaError::DuplicateLabel { name, label } => {
                write!(f, "family `{name}` declares label `{label}` more than once")
            }
            SchemaError::ReservedLabel { name, label } => write!(
                f,
                "family `{name}` declares label `{label}`, which the encoder \
                 owns as a sample-structure label"
            ),
            SchemaError::UnitSuffix { name, unit } => write!(
                f,
                "family `{name}` has unit `{unit}` but its name does not end in `_{unit}`"
            ),
            SchemaError::UndescribedEntry { name } => write!(
                f,
                "entry `{name}` emits values without a declared schema \
                 (a runtime-projected entry cannot describe itself in a dynamic group)"
            ),
        }
    }
}

impl std::error::Error for SchemaError {}

/// How a histogram family prefers its buckets rendered.
///
/// A family-level *declaration*, like [`Unit`]: backends declare their natural
/// form at describe time (a dynamic exponential histogram is only
/// aggregation-sound as non-cumulative `vmrange`; a fixed layout is sound as
/// classic `le`), and the sink resolves the declaration against the
/// exposition's capability (whether the scraper can ingest `vmrange` at all).
///
/// `#[non_exhaustive]`: further renderings may be added without a breaking
/// change, so external `match`es must include a wildcard arm.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum HistogramRender {
    /// No declaration: the sink's document-level default applies (classic
    /// `le` in the OpenMetrics encoder).
    #[default]
    Auto,
    /// Classic cumulative `le` buckets.
    Le,
    /// VictoriaMetrics non-cumulative `vmrange` buckets, when the exposition
    /// allows them; degrades to `le` otherwise.
    VmRange,
}

/// One OpenMetrics family in a [`MetricSchema`].
///
/// `#[non_exhaustive]`: families are built through
/// [`MetricSchema::add_family`] and read field-by-field, so a future
/// OpenMetrics attribute can become a new field without a breaking change.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct MetricFamilySchema {
    /// The family name before type suffixes such as `_total`.
    pub name: String,
    /// The OpenMetrics type.
    pub metric_type: MetricType,
    /// Optional `# HELP` text.
    pub help: Option<Help>,
    /// Optional `# UNIT` value.
    pub unit: Option<Unit>,
    /// Label names the family may emit, sorted for deterministic output.
    pub labels: Vec<String>,
    /// The declared bucket rendering for histogram families
    /// ([`HistogramRender::Auto`] for everything else).
    pub histogram_render: HistogramRender,
}

/// Label names the OpenMetrics encoder owns as sample-structure labels. A
/// family declaration that uses one of them (without structurally owning it,
/// see [`SchemaError::ReservedLabel`]) is recorded for
/// [`validate`](MetricSchema::validate) to report.
const RESERVED_LABELS: [&str; 3] = ["le", "quantile", "vmrange"];

/// Whether a declaration of `metric_type` structurally owns the reserved
/// `label`: gauge histogram samples carry their own `le` pair and summary
/// samples their own `quantile` pair, so their describe paths legitimately
/// declare the name.
fn owns_reserved_label(metric_type: MetricType, label: &str) -> bool {
    matches!(
        (metric_type, label),
        (MetricType::GaugeHistogram, "le") | (MetricType::Summary, "quantile")
    )
}

/// A registry's metric contract.
///
/// # Reserved label names
///
/// The OpenMetrics encoder owns `le`, `quantile`, and `vmrange` as
/// sample-structure labels: it attaches them itself when rendering bucket
/// bounds and quantiles. Declaring one of them on a family through
/// [`add_family`](MetricSchema::add_family) is recorded as a
/// [`SchemaError::ReservedLabel`] (unless the metric type structurally owns
/// the name -- a gauge histogram's `le`, a summary's `quantile`) and reported
/// by [`validate`](MetricSchema::validate). Instruments that compose the
/// owned structural pair use
/// `add_family_structural` so a
/// user/inherited collision is checked before compose.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricSchema {
    /// Families, kept sorted by name so [`families`](MetricSchema::families)
    /// can hand out the slice as-is. New families are inserted at their
    /// sorted position; see `rebuild_index`
    /// for why the name→position map is rebuilt rather than shift-fixed.
    families: Vec<MetricFamilySchema>,
    /// Family name -> position in `families`, so [`family`](MetricSchema::family)
    /// and the add-family existence check are O(1) instead of a linear scan
    /// per lookup (the schema is rebuilt on every scrape).
    index: HashMap<String, usize>,
    metadata: HashMap<String, (Option<Help>, Option<Unit>)>,
    /// Render declarations recorded before their family was added (the
    /// `set_render_for`-then-`add_family` ordering, mirroring `metadata`).
    renders: HashMap<String, HistogramRender>,
    /// Integrity problems recorded while building the schema (type conflicts,
    /// undescribed entries), kept so the scrape path never fails while
    /// [`validate`](MetricSchema::validate) can still surface them.
    errors: Vec<SchemaError>,
}

impl MetricSchema {
    /// Creates an empty schema.
    pub fn new() -> Self {
        MetricSchema::default()
    }

    /// Associates HELP/UNIT metadata with an exact metric family name, before
    /// or after the family is added (the [`set_render_for`](MetricSchema::set_render_for)
    /// works-in-both-orders contract).
    ///
    /// Merges exactly like the single setters ([`set_help_for`] /
    /// [`set_unit_for`]): a `Some` component overrides, a `None` component
    /// leaves any existing value in place -- on both the already-added-family
    /// path and the stashed-metadata path.
    ///
    /// [`set_help_for`]: MetricSchema::set_help_for
    /// [`set_unit_for`]: MetricSchema::set_unit_for
    pub fn set_metadata_for(&mut self, family: &str, help: Option<Help>, unit: Option<Unit>) {
        if help.is_none() && unit.is_none() {
            return;
        }
        if let Some(existing) = self.family_mut(family) {
            if help.is_some() {
                existing.help = help;
            }
            if unit.is_some() {
                existing.unit = unit;
            }
        } else {
            let entry = self
                .metadata
                .entry(family.to_owned())
                .or_insert((None, None));
            if help.is_some() {
                entry.0 = help;
            }
            if unit.is_some() {
                entry.1 = unit;
            }
        }
    }

    /// Associates HELP metadata with an exact metric family name, before or
    /// after the family is added.
    pub fn set_help_for(&mut self, family: &str, help: impl Into<Help>) {
        let help = Some(help.into());
        if let Some(existing) = self.family_mut(family) {
            existing.help = help;
        } else {
            let entry = self
                .metadata
                .entry(family.to_owned())
                .or_insert((None, None));
            entry.0 = help;
        }
    }

    /// Associates UNIT metadata with an exact metric family name, before or
    /// after the family is added.
    pub fn set_unit_for(&mut self, family: &str, unit: impl Into<Unit>) {
        let unit = Some(unit.into());
        if let Some(existing) = self.family_mut(family) {
            existing.unit = unit;
        } else {
            let entry = self
                .metadata
                .entry(family.to_owned())
                .or_insert((None, None));
            entry.1 = unit;
        }
    }

    fn family_mut(&mut self, name: &str) -> Option<&mut MetricFamilySchema> {
        let position = *self.index.get(name)?;
        Some(&mut self.families[position])
    }

    /// Rebuilds `index` from `families`. Chosen over a per-insert shift-fixup
    /// of every map value: same O(F) cost as the walk it replaces, but the map
    /// cannot drift from the vec (the previous coherence hazard). A deferred
    /// append+sort would need interior mutability to keep
    /// [`families`](MetricSchema::families) returning a slice from `&self`;
    /// schema sizes are small enough that sorted `Vec::insert` + rebuild is
    /// the simpler contract-preserving shape.
    fn rebuild_index(&mut self) {
        self.index.clear();
        for (position, family) in self.families.iter().enumerate() {
            self.index.insert(family.name.clone(), position);
        }
    }

    /// Adds or merges one family declaration.
    ///
    /// A declaration that repeats a label name is deduplicated for rendering
    /// and recorded as a [`SchemaError::DuplicateLabel`] for
    /// [`validate`](MetricSchema::validate) to report: the internal label
    /// composition resolves name collisions before they get here (the inner
    /// pair wins), so a repeat in a direct declaration is a caller bug.
    ///
    /// A declaration that uses a reserved sample-structure label name (`le`,
    /// `quantile`, `vmrange`) is recorded as a
    /// [`SchemaError::ReservedLabel`], unless the declared metric type
    /// structurally owns the name (a gauge histogram's `le`, a summary's
    /// `quantile`). Instruments that compose a structural pair on top of
    /// user/inherited labels must use
    /// `add_family_structural` so a
    /// user collision with the owned name is still detected.
    pub fn add_family(&mut self, name: &str, metric_type: MetricType, labels: &[(&str, &str)]) {
        let mut label_names: Vec<String> =
            labels.iter().map(|(name, _)| (*name).to_owned()).collect();
        label_names.sort();
        let mut reported: Option<&str> = None;
        for pair in label_names.windows(2) {
            if pair[0] == pair[1] && reported != Some(pair[0].as_str()) {
                self.errors.push(SchemaError::DuplicateLabel {
                    name: name.to_owned(),
                    label: pair[0].clone(),
                });
                reported = Some(pair[0].as_str());
            }
        }
        label_names.dedup();

        for label in &label_names {
            if RESERVED_LABELS.contains(&label.as_str()) && !owns_reserved_label(metric_type, label)
            {
                self.errors.push(SchemaError::ReservedLabel {
                    name: name.to_owned(),
                    label: label.clone(),
                });
            }
        }

        if let Some(&position) = self.index.get(name) {
            let existing = &mut self.families[position];
            // A family name must map to exactly one metric type. A conflicting
            // re-declaration is a schema bug -- it would emit two different
            // `# TYPE` lines for one name and Prometheus would reject the
            // scrape -- so record it for `validate` to report. The
            // first-registered type is kept (merging only labels) so the
            // scrape still renders one coherent family.
            if existing.metric_type != metric_type {
                self.errors.push(SchemaError::TypeConflict {
                    name: name.to_owned(),
                    kept: existing.metric_type,
                    rejected: metric_type,
                });
            }
            for label in label_names {
                if !existing.labels.contains(&label) {
                    existing.labels.push(label);
                }
            }
            existing.labels.sort();
            return;
        }

        let (help, unit) = self.metadata.remove(name).unwrap_or((None, None));
        let histogram_render = self.renders.remove(name).unwrap_or_default();
        // Keep `families` name-sorted via insert-at-position so `families()`
        // can return the slice as-is; rebuild the index instead of shift-fixup.
        let position = self
            .families
            .partition_point(|family| family.name.as_str() < name);
        self.families.insert(
            position,
            MetricFamilySchema {
                name: name.to_owned(),
                metric_type,
                help,
                unit,
                labels: label_names,
                histogram_render,
            },
        );
        self.rebuild_index();
    }

    /// Adds a family whose instrument owns a structural sample-structure
    /// label (`quantile` for a summary, `le` for a gauge histogram).
    ///
    /// Checks the *incoming* `labels` (user/inherited) for a collision with
    /// the type-owned reserved name **before** composing `structural` —
    /// otherwise label composition would shadow the user pair and
    /// [`add_family`](MetricSchema::add_family)'s type exemption would treat
    /// the composed declaration as legitimate. Other reserved names in the
    /// incoming slice are still reported by the subsequent `add_family` call.
    pub(crate) fn add_family_structural(
        &mut self,
        name: &str,
        metric_type: MetricType,
        labels: &[(&str, &str)],
        structural: &[(&str, &str)],
    ) {
        // Provenance: flag a user/inherited label the type structurally owns
        // before compose erases the evidence. Dedup by name so a repeated
        // incoming pair records one error (matching DuplicateLabel).
        let mut reported: Option<&str> = None;
        for (label, _) in labels {
            if owns_reserved_label(metric_type, label) && reported != Some(*label) {
                self.errors.push(SchemaError::ReservedLabel {
                    name: name.to_owned(),
                    label: (*label).to_owned(),
                });
                reported = Some(*label);
            }
        }
        let all = with_labels(labels, structural.iter().copied());
        self.add_family(name, metric_type, &all);
    }

    /// Declares the bucket rendering for a histogram `family`, before or after
    /// the family is added (the [`set_help_for`](MetricSchema::set_help_for) /
    /// [`set_unit_for`](MetricSchema::set_unit_for) style).
    pub fn set_render_for(&mut self, family: &str, render: HistogramRender) {
        if let Some(existing) = self.family_mut(family) {
            existing.histogram_render = render;
        } else {
            self.renders.insert(family.to_owned(), render);
        }
    }

    /// Adds a family whose labels are `const_labels` (carrying values) plus
    /// `declared` extra label names (value-less). This is what reader/opaque
    /// registry entries need: they declare their label dimension separately
    /// from the sampled value.
    pub(crate) fn add_family_with_const_labels(
        &mut self,
        name: &str,
        metric_type: MetricType,
        const_labels: &[(&str, &str)],
        declared: &[String],
    ) {
        let all = with_labels(
            const_labels,
            declared.iter().map(|label| (label.as_str(), "")),
        );
        self.add_family(name, metric_type, &all);
    }

    /// Records an integrity problem found while building the schema, surfaced
    /// through [`validate`](MetricSchema::validate) rather than failing the
    /// scrape.
    pub(crate) fn record_error(&mut self, error: SchemaError) {
        self.errors.push(error);
    }

    /// All metric families, sorted by family name.
    pub fn families(&self) -> &[MetricFamilySchema] {
        &self.families
    }

    /// Finds one metric family by exact name.
    pub fn family(&self, name: &str) -> Option<&MetricFamilySchema> {
        let position = *self.index.get(name)?;
        Some(&self.families[position])
    }

    /// Checks the schema for integrity problems: metric-type conflicts and
    /// undescribed dynamic-group entries recorded while building the schema,
    /// and families whose declared unit does not suffix their name. Returns
    /// every problem found.
    ///
    /// This is the deliberate, fallible integrity check: building a schema
    /// never fails (a scrape must always render; the encoder suppresses an
    /// invalid `# UNIT` line as a last resort), so run this once when a service
    /// assembles its metrics -- a boot-time assertion or a test -- to catch a
    /// contract violation before it ships.
    pub fn validate(&self) -> Result<(), Vec<SchemaError>> {
        let mut errors = self.errors.clone();
        for family in &self.families {
            if let Some(unit) = &family.unit {
                let token = unit.as_str();
                if !name_has_unit_suffix(&family.name, token) {
                    errors.push(SchemaError::UnitSuffix {
                        name: family.name.clone(),
                        unit: token.to_owned(),
                    });
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Whether `name` satisfies the OpenMetrics unit-suffix rule for `unit`: an
/// empty unit has nothing to check, and otherwise the unit must be the whole
/// name or an `_`-separated suffix of it (a `seconds` unit needs a `_seconds`
/// name). This is the check behind [`SchemaError::UnitSuffix`], shared here so
/// sinks and validators apply the same rule without allocating.
pub fn name_has_unit_suffix(name: &str, unit: &str) -> bool {
    if unit.is_empty() || name == unit {
        return true;
    }
    // Byte-wise suffix check: `unit` tokens are ASCII OpenMetrics names, so
    // the separator test on the preceding byte is exact and allocation-free.
    name.len() > unit.len()
        && name.ends_with(unit)
        && name.as_bytes()[name.len() - unit.len() - 1] == b'_'
}

#[cfg(test)]
mod tests {
    use crate::{Help, Unit};

    use super::*;

    #[test]
    fn metadata_applies_once_to_the_exact_family() {
        let mut schema = MetricSchema::new();
        schema.set_metadata_for(
            "requests",
            Some(Help::from("Total requests")),
            Some(Unit::Requests),
        );
        schema.add_family(
            "requests_by_route",
            MetricType::Counter,
            &[("route", "/health")],
        );
        schema.add_family("requests", MetricType::Counter, &[("service", "api")]);

        let child = schema.family("requests_by_route").unwrap();
        assert_eq!(child.help, None);
        assert_eq!(child.unit, None);

        let exact = schema.family("requests").unwrap();
        assert_eq!(
            exact.help.as_ref().map(Help::as_str),
            Some("Total requests")
        );
        assert_eq!(exact.unit.as_ref().map(Unit::as_str), Some("requests"));
    }

    #[test]
    fn metadata_setters_work_before_and_after_add_family() {
        // Before: metadata recorded first is consumed when the family arrives.
        let mut before = MetricSchema::new();
        before.set_help_for("requests", "Total requests");
        before.set_unit_for("requests", Unit::Requests);
        before.add_family("requests", MetricType::Counter, &[]);
        let family = before.family("requests").unwrap();
        assert_eq!(
            family.help.as_ref().map(Help::as_str),
            Some("Total requests")
        );
        assert_eq!(family.unit.as_ref().map(Unit::as_str), Some("requests"));

        // After: an already-added family is updated in place (previously a
        // silent no-op, unlike `set_render_for`).
        let mut after = MetricSchema::new();
        after.add_family("requests", MetricType::Counter, &[]);
        after.set_help_for("requests", "Total requests");
        after.set_unit_for("requests", Unit::Requests);
        after.set_metadata_for(
            "requests",
            Some(Help::from("Requests served")),
            Some(Unit::Requests),
        );
        let family = after.family("requests").unwrap();
        assert_eq!(
            family.help.as_ref().map(Help::as_str),
            Some("Requests served")
        );
        assert_eq!(family.unit.as_ref().map(Unit::as_str), Some("requests"));
    }

    #[test]
    fn set_metadata_for_merges_like_the_single_setters_on_an_added_family() {
        // A `None` component must not erase a value the single setters put in
        // place; a `Some` component overrides it.
        let mut schema = MetricSchema::new();
        schema.add_family("requests", MetricType::Counter, &[]);
        schema.set_help_for("requests", "Total requests");
        schema.set_metadata_for("requests", None, Some(Unit::Requests));
        let family = schema.family("requests").unwrap();
        assert_eq!(
            family.help.as_ref().map(Help::as_str),
            Some("Total requests"),
            "None help must not clobber the existing help"
        );
        assert_eq!(family.unit.as_ref().map(Unit::as_str), Some("requests"));

        schema.set_metadata_for("requests", Some(Help::from("Requests served")), None);
        let family = schema.family("requests").unwrap();
        assert_eq!(
            family.help.as_ref().map(Help::as_str),
            Some("Requests served"),
            "Some help overrides"
        );
        assert_eq!(
            family.unit.as_ref().map(Unit::as_str),
            Some("requests"),
            "None unit must not clobber the existing unit"
        );
    }

    #[test]
    fn set_metadata_for_merges_like_the_single_setters_on_the_stash_path() {
        // Same contract before the family exists: partial `set_metadata_for`
        // calls and single setters merge into one stash entry, in either
        // order, and the family consumes the merged result when it arrives.
        let mut schema = MetricSchema::new();
        schema.set_help_for("requests", "Total requests");
        schema.set_metadata_for("requests", None, Some(Unit::Requests));
        schema.add_family("requests", MetricType::Counter, &[]);
        let family = schema.family("requests").unwrap();
        assert_eq!(
            family.help.as_ref().map(Help::as_str),
            Some("Total requests"),
            "the stashed help must survive a help-less set_metadata_for"
        );
        assert_eq!(family.unit.as_ref().map(Unit::as_str), Some("requests"));

        // And the reverse order: set_metadata_for first, single setter after.
        let mut reversed = MetricSchema::new();
        reversed.set_metadata_for("requests", Some(Help::from("Total requests")), None);
        reversed.set_unit_for("requests", Unit::Requests);
        reversed.add_family("requests", MetricType::Counter, &[]);
        let family = reversed.family("requests").unwrap();
        assert_eq!(
            family.help.as_ref().map(Help::as_str),
            Some("Total requests")
        );
        assert_eq!(family.unit.as_ref().map(Unit::as_str), Some("requests"));
    }

    #[test]
    fn validate_flags_a_declaration_with_duplicate_label_names() {
        let mut schema = MetricSchema::new();
        schema.add_family(
            "requests",
            MetricType::Counter,
            &[("method", "get"), ("method", "post"), ("route", "/")],
        );

        // Rendering still sees each name once.
        assert_eq!(
            schema.family("requests").unwrap().labels,
            vec!["method", "route"]
        );
        // The duplicated declaration is reported exactly once per name.
        assert_eq!(
            schema.validate().unwrap_err(),
            vec![SchemaError::DuplicateLabel {
                name: "requests".to_owned(),
                label: "method".to_owned(),
            }]
        );
    }

    #[test]
    fn validate_flags_a_declaration_with_a_reserved_label_name() {
        let mut schema = MetricSchema::new();
        schema.add_family(
            "requests",
            MetricType::Counter,
            &[("le", "0.5"), ("route", "/")],
        );
        assert_eq!(
            schema.validate().unwrap_err(),
            vec![SchemaError::ReservedLabel {
                name: "requests".to_owned(),
                label: "le".to_owned(),
            }]
        );

        // `vmrange` and `quantile` are reserved on types that do not own them.
        let mut vmrange = MetricSchema::new();
        vmrange.add_family("latency_seconds", MetricType::Histogram, &[("vmrange", "")]);
        assert_eq!(
            vmrange.validate().unwrap_err(),
            vec![SchemaError::ReservedLabel {
                name: "latency_seconds".to_owned(),
                label: "vmrange".to_owned(),
            }]
        );
    }

    #[test]
    fn structural_owners_of_reserved_labels_pass_validate() {
        // Direct add_family of the structural dimension is the exemption.
        let mut schema = MetricSchema::new();
        schema.add_family("queue_depth", MetricType::GaugeHistogram, &[("le", "")]);
        schema.add_family("latency", MetricType::Summary, &[("quantile", "")]);
        assert!(schema.validate().is_ok(), "{:?}", schema.validate());

        // The exemption is per name: a summary declaring `le` is still flagged.
        let mut wrong = MetricSchema::new();
        wrong.add_family("latency", MetricType::Summary, &[("le", "")]);
        assert_eq!(
            wrong.validate().unwrap_err(),
            vec![SchemaError::ReservedLabel {
                name: "latency".to_owned(),
                label: "le".to_owned(),
            }]
        );
    }

    #[test]
    fn structural_path_flags_incoming_collision_with_owned_reserved_label() {
        // User/inherited labels colliding with the type-owned name are caught
        // before compose; a plain histogram with user `le` is still flagged.
        let mut summary = MetricSchema::new();
        summary.add_family_structural(
            "latency",
            MetricType::Summary,
            &[("quantile", "0.5"), ("svc", "api")],
            &[("quantile", "")],
        );
        assert_eq!(
            summary.validate().unwrap_err(),
            vec![SchemaError::ReservedLabel {
                name: "latency".to_owned(),
                label: "quantile".to_owned(),
            }]
        );

        let mut gauge_hist = MetricSchema::new();
        gauge_hist.add_family_structural(
            "queue_depth",
            MetricType::GaugeHistogram,
            &[("le", "10")],
            &[("le", "")],
        );
        assert_eq!(
            gauge_hist.validate().unwrap_err(),
            vec![SchemaError::ReservedLabel {
                name: "queue_depth".to_owned(),
                label: "le".to_owned(),
            }]
        );

        let mut histogram = MetricSchema::new();
        histogram.add_family("latency_seconds", MetricType::Histogram, &[("le", "")]);
        assert_eq!(
            histogram.validate().unwrap_err(),
            vec![SchemaError::ReservedLabel {
                name: "latency_seconds".to_owned(),
                label: "le".to_owned(),
            }]
        );

        // Clean structural compose (no incoming collision) stays green.
        let mut clean = MetricSchema::new();
        clean.add_family_structural(
            "latency",
            MetricType::Summary,
            &[("svc", "api")],
            &[("quantile", "")],
        );
        clean.add_family_structural(
            "queue_depth",
            MetricType::GaugeHistogram,
            &[("shard", "a")],
            &[("le", "")],
        );
        assert!(clean.validate().is_ok(), "{:?}", clean.validate());
    }

    #[test]
    fn family_index_stays_coherent_across_inserts_and_metadata() {
        // Families arrive out of name order, so each insert shifts earlier
        // entries' positions; lookups and the before/after metadata setters
        // must keep finding the right family afterwards.
        let mut schema = MetricSchema::new();
        schema.set_render_for("a_latency", HistogramRender::VmRange); // stashed
        schema.add_family("m_requests", MetricType::Counter, &[("route", "/")]);
        schema.set_help_for("m_requests", "Requests"); // after add
        schema.add_family("a_latency", MetricType::Histogram, &[]); // shifts m_requests
        schema.add_family("z_depth", MetricType::Gauge, &[]);
        schema.add_family("f_bytes", MetricType::Counter, &[]); // shifts m_requests, z_depth
        schema.set_help_for("z_depth", "Depth"); // after shift
        schema.set_render_for("a_latency", HistogramRender::Le); // after shift
        schema.add_family("m_requests", MetricType::Counter, &[("status", "200")]); // merge

        let names: Vec<&str> = schema
            .families()
            .iter()
            .map(|family| family.name.as_str())
            .collect();
        assert_eq!(names, vec!["a_latency", "f_bytes", "m_requests", "z_depth"]);

        let latency = schema.family("a_latency").unwrap();
        assert_eq!(latency.metric_type, MetricType::Histogram);
        assert_eq!(latency.histogram_render, HistogramRender::Le);

        let requests = schema.family("m_requests").unwrap();
        assert_eq!(requests.labels, vec!["route", "status"]);
        assert_eq!(requests.help.as_ref().map(Help::as_str), Some("Requests"));

        let depth = schema.family("z_depth").unwrap();
        assert_eq!(depth.help.as_ref().map(Help::as_str), Some("Depth"));

        assert!(schema.family("missing").is_none());
        assert!(schema.validate().is_ok(), "{:?}", schema.validate());
    }

    #[test]
    fn add_family_merges_and_sorts_label_names() {
        let mut schema = MetricSchema::new();
        schema.add_family(
            "requests",
            MetricType::Counter,
            &[("route", "/a"), ("service", "api")],
        );
        schema.add_family(
            "requests",
            MetricType::Counter,
            &[("status", "500"), ("route", "/b")],
        );

        assert_eq!(
            schema.family("requests").unwrap().labels,
            vec!["route", "service", "status"]
        );
    }

    #[test]
    fn validate_reports_type_conflicts_without_failing_the_scrape() {
        let mut schema = MetricSchema::new();
        schema.add_family("throughput", MetricType::Counter, &[]);
        schema.add_family("throughput", MetricType::Gauge, &[]);

        // The first type wins so the family still renders coherently.
        assert_eq!(
            schema.family("throughput").unwrap().metric_type,
            MetricType::Counter
        );
        assert_eq!(
            schema.validate().unwrap_err(),
            vec![SchemaError::TypeConflict {
                name: "throughput".to_owned(),
                kept: MetricType::Counter,
                rejected: MetricType::Gauge,
            }]
        );
    }

    #[test]
    fn validate_requires_the_unit_to_suffix_the_family_name() {
        let mut schema = MetricSchema::new();
        schema.set_unit_for("request_latency", Unit::Seconds);
        schema.add_family("request_latency", MetricType::Histogram, &[]);
        assert_eq!(
            schema.validate().unwrap_err(),
            vec![SchemaError::UnitSuffix {
                name: "request_latency".to_owned(),
                unit: "seconds".to_owned(),
            }]
        );

        let mut suffixed = MetricSchema::new();
        suffixed.set_unit_for("request_latency_seconds", Unit::Seconds);
        suffixed.add_family("request_latency_seconds", MetricType::Histogram, &[]);
        assert!(suffixed.validate().is_ok());

        // A glued (non-`_`-separated) suffix is not a unit suffix: the unit
        // cannot be recovered from the series name.
        let mut glued = MetricSchema::new();
        glued.set_unit_for("latencyseconds", Unit::Seconds);
        glued.add_family("latencyseconds", MetricType::Histogram, &[]);
        assert_eq!(
            glued.validate().unwrap_err(),
            vec![SchemaError::UnitSuffix {
                name: "latencyseconds".to_owned(),
                unit: "seconds".to_owned(),
            }]
        );
    }

    #[test]
    fn name_has_unit_suffix_matches_the_openmetrics_rule() {
        // Whole-name match, `_`-separated suffix, and the empty unit all pass.
        assert!(name_has_unit_suffix("seconds", "seconds"));
        assert!(name_has_unit_suffix("call_duration_seconds", "seconds"));
        assert!(name_has_unit_suffix("app_requests", "requests"));
        assert!(name_has_unit_suffix("queue_depth", ""));
        // A bare suffix without the `_` separator, or no suffix at all, fails.
        assert!(!name_has_unit_suffix("queue_depth", "items"));
        assert!(!name_has_unit_suffix("latencyseconds", "seconds"));
        // The unit alone never matches a shorter or unrelated name.
        assert!(!name_has_unit_suffix("s", "seconds"));
    }
}
