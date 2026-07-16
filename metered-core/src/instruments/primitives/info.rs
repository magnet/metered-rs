use parking_lot::RwLock;

/// Label pairs for [`Info`] metrics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Labels(Vec<(String, String)>);

impl Labels {
    /// Builds labels from key/value pairs.
    pub fn new<I, K, V>(labels: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Labels(
            labels
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }

    /// The label pairs.
    pub fn as_slice(&self) -> &[(String, String)] {
        &self.0
    }
}

impl<I, K, V> From<I> for Labels
where
    I: IntoIterator<Item = (K, V)>,
    K: Into<String>,
    V: Into<String>,
{
    fn from(labels: I) -> Self {
        Labels::new(labels)
    }
}

/// An OpenMetrics `info` value: key/value metadata exported as a single series
/// with value `1` (e.g. build info: `version`, `commit`).
pub trait Info {
    /// The current labels.
    fn labels(&self) -> Labels;

    /// Replaces the labels. Implementations that expose computed/static info
    /// may ignore this; the concrete [`InfoMetric`] is runtime-mutable.
    fn set_labels(&self, labels: Labels) {
        let _ = labels;
    }
}

/// Runtime-mutable concrete [`Info`] metric.
#[derive(Debug, Default)]
pub struct InfoMetric {
    labels: RwLock<Labels>,
}

impl InfoMetric {
    /// Builds an info metric from a set of static label pairs.
    pub fn new(labels: impl Into<Labels>) -> Self {
        InfoMetric {
            labels: RwLock::new(labels.into()),
        }
    }
}

impl Info for InfoMetric {
    fn labels(&self) -> Labels {
        self.labels.read().clone()
    }

    fn set_labels(&self, labels: Labels) {
        *self.labels.write() = labels;
    }
}

impl<T: Info + ?Sized> Info for &T {
    fn labels(&self) -> Labels {
        (**self).labels()
    }

    fn set_labels(&self, labels: Labels) {
        (**self).set_labels(labels);
    }
}

/// Explicitly exposes a value as an OpenMetrics `info` metric.
///
/// Wraps any [`Info`] so it participates in a [`Metric`](crate::Metric) /
/// [`MetricTree`](crate::MetricTree) walk: intrinsic labels compose over
/// enclosing ones through [`compose_labels`](crate::compose_labels)
/// (inner-wins on collision), and the collected series is named
/// `{name}_info`. Unlike gauges and counters, info leaves have no structural
/// upkeep -- [`Metric::housekeep`](crate::Metric::housekeep) /
/// [`Metric::needs_housekeep`](crate::Metric::needs_housekeep) stay at their
/// no-op defaults (the `#[derive(MetricTree)]` `info` field path mirrors that
/// asymmetry and does not forward housekeep to the field).
///
/// The derive's `#[metrics(info)]` field attribute emits `AsInfo` for the same
/// reason `#[metric(gauge)]` / `#[metric(counter)]` emit [`AsGauge`] /
/// [`AsCounter`]: so hand-written trees can force the same leaf path without
/// going through the derive.
///
/// [`AsGauge`]: crate::AsGauge
/// [`AsCounter`]: crate::AsCounter
#[derive(Clone, Copy, Debug)]
pub struct AsInfo<T>(pub T);

impl<T> From<T> for AsInfo<T> {
    fn from(info: T) -> Self {
        AsInfo(info)
    }
}

impl<T: Info> Info for AsInfo<T> {
    fn labels(&self) -> Labels {
        self.0.labels()
    }

    fn set_labels(&self, labels: Labels) {
        self.0.set_labels(labels);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Metric, MetricSchema, MetricValues};

    #[test]
    fn as_info_wraps_an_info_value_for_hand_written_trees() {
        let build = InfoMetric::new([("version", "1.2.3"), ("service", "api")]);
        let wrapped = AsInfo::from(&build);

        assert_eq!(
            Info::labels(&wrapped).as_slice(),
            &[
                ("version".to_owned(), "1.2.3".to_owned()),
                ("service".to_owned(), "api".to_owned()),
            ]
        );

        let mut values = MetricValues::new();
        Metric::collect_metric(&wrapped, "build", &[("service", "registry")], &mut values);
        let sample = values
            .samples()
            .iter()
            .find(|s| s.name == "build_info")
            .expect("info series");
        assert_eq!(sample.value.to_string(), "1");
        assert_eq!(
            sample.labels,
            vec![
                ("service".to_owned(), "api".to_owned()),
                ("version".to_owned(), "1.2.3".to_owned()),
            ]
        );

        let mut schema = MetricSchema::new();
        Metric::describe_metric(&wrapped, "build", &[("service", "registry")], &mut schema);
        let family = schema.family("build").expect("info family");
        assert_eq!(
            family.labels.iter().filter(|l| *l == "service").count(),
            1,
            "schema labels: {:?}",
            family.labels
        );
    }
}
