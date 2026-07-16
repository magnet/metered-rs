use metered::entry::counter;
use metered::{LabelSet, MetricTreeView, MetricsView};
use std::collections::HashMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

// A typed, multi-label key for a dynamic set of components. `family_view` is
// the borrowed dual of `Family<RailLabels, ...>`: your own map is the storage
// and the metrics live in the members. Deriving `LabelSet` declares the label
// names statically, which `family_view` needs for its context-free schema.
#[derive(Clone, PartialEq, Eq, Hash, LabelSet)]
pub struct RailLabels {
    rail: String,
    direction: String,
}

pub struct Rail {
    sent: AtomicU64,
}

impl MetricsView for Rail {
    fn metrics_view() -> MetricTreeView<'static, Self> {
        let mut view = MetricTreeView::new();
        view.register(
            counter("sent")
                .select(|rail: &Rail| &rail.sent)
                .help("Payments sent on this rail"),
        );
        view
    }
}

pub struct Rails {
    map: RwLock<HashMap<RailLabels, Rail>>,
}

pub fn rails_view() -> MetricTreeView<'static, Rails> {
    let mut view = MetricTreeView::with_prefix("rails");
    // `iterate` owns the lock scope; every emitted member's series carry the
    // key's label pairs (rail="...", direction="...").
    view.family_view(Rail::metrics_view(), |rails: &Rails, out| {
        for (key, rail) in rails.map.read().unwrap().iter() {
            out.emit(key, rail);
        }
    });
    view
}

pub fn record(rails: &Rails, key: &RailLabels) {
    if let Some(rail) = rails.map.read().unwrap().get(key) {
        rail.sent.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct NamedRails {
    map: RwLock<HashMap<String, Rail>>,
}

// `family_by` is `family_view` for the common one-string-key case: members
// keyed by one plain string, emitted as `rail="<key>"` on each series.
pub fn named_rails_view() -> MetricTreeView<'static, NamedRails> {
    let mut view = MetricTreeView::with_prefix("rails");
    view.family_by("rail", Rail::metrics_view(), |rails: &NamedRails, out| {
        for (name, rail) in rails.map.read().unwrap().iter() {
            out.emit(name, rail);
        }
    });
    view
}
