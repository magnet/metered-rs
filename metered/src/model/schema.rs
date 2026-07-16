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
    /// An entry could not describe its schema in a context-free walk (a
    /// dynamic [`each`](crate::MetricTreeView::each) group): it resolves its
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

/// One OpenMetrics family in a [`MetricSchema`].
#[derive(Clone, Debug, PartialEq, Eq)]
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
}

/// A registry's metric contract.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricSchema {
    families: Vec<MetricFamilySchema>,
    metadata: HashMap<String, (Option<Help>, Option<Unit>)>,
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

    /// Associates HELP/UNIT metadata with an exact metric family name.
    pub fn set_metadata_for(&mut self, family: &str, help: Option<Help>, unit: Option<Unit>) {
        if help.is_some() || unit.is_some() {
            self.metadata.insert(family.to_owned(), (help, unit));
        }
    }

    /// Associates HELP metadata with an exact metric family name.
    pub fn set_help_for(&mut self, family: &str, help: impl Into<Help>) {
        let entry = self
            .metadata
            .entry(family.to_owned())
            .or_insert((None, None));
        entry.0 = Some(help.into());
    }

    /// Associates UNIT metadata with an exact metric family name.
    pub fn set_unit_for(&mut self, family: &str, unit: impl Into<Unit>) {
        let entry = self
            .metadata
            .entry(family.to_owned())
            .or_insert((None, None));
        entry.1 = Some(unit.into());
    }

    /// Adds or merges one family declaration.
    pub fn add_family(&mut self, name: &str, metric_type: MetricType, labels: &[(&str, &str)]) {
        let mut label_names: Vec<String> =
            labels.iter().map(|(name, _)| (*name).to_owned()).collect();
        label_names.sort();
        label_names.dedup();

        if let Some(existing) = self.families.iter_mut().find(|family| family.name == name) {
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
        self.families.push(MetricFamilySchema {
            name: name.to_owned(),
            metric_type,
            help,
            unit,
            labels: label_names,
        });
        self.families.sort_by(|a, b| a.name.cmp(&b.name));
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
        self.families.iter().find(|family| family.name == name)
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
                let suffixed = family.name == token || family.name.ends_with(&format!("_{token}"));
                if !suffixed {
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
    }
}
