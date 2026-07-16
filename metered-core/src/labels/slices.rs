//! Small typed helpers for the borrowed label slices threaded through
//! `describe` / `collect` / `encode`.

/// Returns the enclosing `outer` labels with the more-specific `inner` pairs
/// appended, resolving name collisions with the inner-wins policy.
///
/// This is the one allocation a metric makes when it adds its own label
/// dimensions -- an `le` bucket bound, a `quantile`, a stateset state, an
/// `Info` fact, a family key's pairs -- on top of the constant labels it
/// inherits.
///
/// # Name-collision policy: the inner pair wins
///
/// One composed slice must never carry two pairs with one label name:
/// OpenMetrics forbids duplicate label names in a series, and the schema
/// deduplicates *names* while the values would keep both *pairs*. So
/// composition is name-keyed, and on a collision the **inner** pair wins --
/// the `inner` pair, appended later and more specific to the metric --
/// replacing the inherited pair in place (the slice keeps the outer pair's
/// position, so label order stays deterministic). A registry constant label
/// shadowed by a family's key label, an `le` bound, or an `Info` fact
/// therefore drops out of that series deterministically.
///
/// # Who must use this
///
/// The policy holds end to end -- for both `describe` and `collect` -- only
/// while every site that composes enclosing labels with more-specific ones
/// funnels through this function. Raw concatenation reintroduces duplicate
/// names. Known consumers of the contract:
///
/// * the composition sites inside this crate (tree walks, families,
///   instruments that add structural pairs such as `le` or `quantile`),
/// * `metered-macro` derive expansions that compose container labels and
///   `Info` labels (reaching this function as `::metered::compose_labels`
///   through the facade),
/// * sink-side tree composition such as `metered-om`'s `TextSourceTree`.
///
/// # Reserved label names
///
/// The OpenMetrics encoder owns `le`, `quantile`, and `vmrange` as
/// sample-structure labels. Composing one of them here follows the same
/// inner-wins rule, but *declaring* one on a family that does not
/// structurally own it is a schema bug --
/// [`MetricSchema::validate`](crate::schema::MetricSchema::validate) reports
/// it as a [`SchemaError::ReservedLabel`](crate::schema::SchemaError).
pub fn compose_labels<'a>(
    outer: &[(&'a str, &'a str)],
    inner: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Vec<(&'a str, &'a str)> {
    let mut all = outer.to_vec();
    for (name, value) in inner {
        match all.iter_mut().find(|(existing, _)| *existing == name) {
            Some(pair) => *pair = (name, value),
            None => all.push((name, value)),
        }
    }
    all
}

/// Crate-internal alias for [`compose_labels`], keeping the established name
/// at the in-crate composition sites. One implementation.
pub(crate) use compose_labels as with_labels;

/// Clones a borrowed label slice into owned pairs, for storage in a sampled
/// [`MetricSample`](crate::values::MetricSample).
pub(crate) fn clone_labels(labels: &[(&str, &str)]) -> Vec<(String, String)> {
    labels
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_labels_appends_disjoint_names_in_order() {
        let outer = [("service", "api"), ("env", "test")];
        let all = compose_labels(&outer, [("route", "/health")]);
        assert_eq!(
            all,
            vec![("service", "api"), ("env", "test"), ("route", "/health")]
        );
    }

    #[test]
    fn compose_labels_lets_the_inner_pair_win_a_name_collision() {
        // The inherited (outer) pair is replaced in place: exactly one pair
        // per name survives, and the inner (more specific) value wins.
        let outer = [("service", "api"), ("method", "outer")];
        let all = compose_labels(&outer, [("method", "get"), ("route", "/health")]);
        assert_eq!(
            all,
            vec![("service", "api"), ("method", "get"), ("route", "/health")]
        );
    }

    #[test]
    fn compose_labels_dedupes_collisions_within_inner_itself() {
        let all = compose_labels(&[], [("state", "a"), ("state", "b")]);
        assert_eq!(all, vec![("state", "b")]);
    }
}
