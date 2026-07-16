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
