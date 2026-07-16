//! Small typed helpers for the borrowed label slices threaded through
//! `describe` / `collect` / `encode`.

/// Returns the inherited `base` labels with `extra` pairs appended.
///
/// This is the one allocation a metric makes when it adds its own label
/// dimensions -- an `le` bucket bound, a `quantile`, a stateset state, an
/// `Info` fact -- on top of the constant labels it inherits.
pub(crate) fn with_labels<'a>(
    base: &[(&'a str, &'a str)],
    extra: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<(&'a str, &'a str)> {
    let mut all = base.to_vec();
    all.extend(extra);
    all
}

/// Clones a borrowed label slice into owned pairs, for storage in a sampled
/// [`MetricSample`](crate::values::MetricSample).
pub(crate) fn clone_labels(labels: &[(&str, &str)]) -> Vec<(String, String)> {
    labels
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}
