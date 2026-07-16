use super::{index_of, ExponentialSnapshot, MAX_SCHEMA, MIN_SCHEMA, ZERO_BUCKET};
use crate::bucket_histogram::{Exemplar, HistogramSnapshot};
use crate::instruments::atomic_f64::AtomicF64;
use arc_swap::{ArcSwap, ArcSwapOption};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const EMPTY_SLOT: i64 = i64::MIN;

/// Per-bucket exemplar window state, in one atomic so the illegal
/// "interesting-but-not-taken" combination is unrepresentable and the
/// adoption/upgrade eligibility transitions are exactly-once via CAS. The
/// exemplar value itself is still best-effort under races (last store wins).
const WINDOW_OPEN: u8 = 0;
const WINDOW_TAKEN: u8 = 1;
const WINDOW_TAKEN_INTERESTING: u8 = 2;

struct Slot {
    index: AtomicI64,
    count: AtomicU64,
    /// The bucket's sampled exemplar, swapped lock-free.
    exemplar: ArcSwapOption<Exemplar>,
    /// Per-window adoption state: `WINDOW_OPEN` (eligible), `WINDOW_TAKEN`
    /// (a non-interesting exemplar stands) or `WINDOW_TAKEN_INTERESTING`.
    window: AtomicU8,
}

impl Slot {
    fn empty() -> Self {
        Slot {
            index: AtomicI64::new(EMPTY_SLOT),
            count: AtomicU64::new(0),
            exemplar: ArcSwapOption::empty(),
            window: AtomicU8::new(WINDOW_OPEN),
        }
    }

    /// Adopts `exemplar` as this bucket's sample if eligible this window: the
    /// window is open (first-in-window), or `interesting` upgrades a standing
    /// non-interesting one. The eligibility transition is a single CAS, so
    /// first-in-window and the upgrade are exactly-once even under racing
    /// observers; the stored exemplar value is best-effort last-store-wins if
    /// a writer is delayed across a reopen. Returns whether this call won an
    /// eligibility transition.
    fn offer_exemplar(&self, exemplar: &Exemplar, interesting: bool) -> bool {
        let desired = if interesting {
            WINDOW_TAKEN_INTERESTING
        } else {
            WINDOW_TAKEN
        };
        loop {
            let current = self.window.load(Ordering::Acquire);
            let eligible = current == WINDOW_OPEN || (interesting && current == WINDOW_TAKEN);
            if !eligible {
                return false;
            }
            if self
                .window
                .compare_exchange_weak(current, desired, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                self.exemplar.store(Some(Arc::new(exemplar.clone())));
                return true;
            }
        }
    }

    /// Reopens the bucket for a fresh exemplar next window, keeping the current
    /// value so the imminent scrape still reads it.
    fn reopen(&self) {
        self.window.store(WINDOW_OPEN, Ordering::Release);
    }
}

/// A fixed-capacity, open-addressed, lock-free table of bucket index -> count at
/// a single schema. New buckets are claimed with a CAS; lookups and increments
/// are plain atomics. When full, `add` reports failure so the caller can fold
/// into an overflow bucket and request a downscale.
struct Table {
    schema: i32,
    mask: usize,
    populated: AtomicUsize,
    slots: Box<[Slot]>,
}

impl Table {
    fn new(capacity: usize, schema: i32) -> Self {
        let capacity = capacity.next_power_of_two().max(8);
        let slots = (0..capacity)
            .map(|_| Slot::empty())
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Table {
            schema,
            mask: capacity - 1,
            populated: AtomicUsize::new(0),
            slots,
        }
    }

    fn home(&self, index: i32) -> usize {
        let mixed = (index as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        (mixed >> 32) as usize & self.mask
    }

    /// Finds (or claims, on first touch) the slot for `index`. Returns `None`
    /// only if the table is full and `index` is not already present.
    fn slot_for(&self, index: i32) -> Option<&Slot> {
        let home = self.home(index);
        for probe in 0..self.slots.len() {
            let slot = &self.slots[(home + probe) & self.mask];
            let cur = slot.index.load(Ordering::Acquire);
            if cur == index as i64 {
                return Some(slot);
            }
            if cur == EMPTY_SLOT {
                match slot.index.compare_exchange(
                    EMPTY_SLOT,
                    index as i64,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        self.populated.fetch_add(1, Ordering::Relaxed);
                        return Some(slot);
                    }
                    Err(actual) if actual == index as i64 => return Some(slot),
                    Err(_) => continue,
                }
            }
        }
        None
    }

    /// Adds `n` to bucket `index`, claiming a slot on first touch. Returns
    /// `false` if the table is full and the bucket is not already present.
    fn add(&self, index: i32, n: u64) -> bool {
        match self.slot_for(index) {
            Some(slot) => {
                slot.count.fetch_add(n, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Increments bucket `index` by one, claiming a slot on first touch. When
    /// `offer` is `Some`, the exemplar is adopted per the window rule (see
    /// [`Slot::offer_exemplar`]). Returns `(added, adopted)`: `added` is `false`
    /// only when the table is full and the bucket is not already present.
    fn offer(&self, index: i32, offer: Option<(&Exemplar, bool)>) -> (bool, bool) {
        match self.slot_for(index) {
            Some(slot) => {
                slot.count.fetch_add(1, Ordering::Relaxed);
                let adopted = match offer {
                    Some((exemplar, interesting)) => slot.offer_exemplar(exemplar, interesting),
                    None => false,
                };
                (true, adopted)
            }
            None => (false, false),
        }
    }

    /// Reopens every populated bucket for a fresh exemplar next window.
    fn reopen_all(&self) {
        for slot in self.slots.iter() {
            if slot.index.load(Ordering::Acquire) != EMPTY_SLOT {
                slot.reopen();
            }
        }
    }

    /// Carries a bucket's exemplar to its post-downscale slot (best-effort: a
    /// full table simply drops it, mirroring the count's overflow handling).
    fn migrate_exemplar(&self, index: i32, exemplar: Arc<Exemplar>) {
        if let Some(slot) = self.slot_for(index) {
            slot.exemplar.store(Some(exemplar));
        }
    }

    /// Total observations across all slots, without allocating (empty slots
    /// hold a zero count, so summing every slot is correct).
    fn total(&self) -> u64 {
        self.slots
            .iter()
            .map(|slot| slot.count.load(Ordering::Relaxed))
            .sum()
    }

    /// The populated `(index, count, exemplar)` triples, in arbitrary order.
    fn populated_with_exemplars(&self) -> Vec<(i32, u64, Option<Exemplar>)> {
        let mut out = Vec::with_capacity(self.populated.load(Ordering::Relaxed));
        for slot in self.slots.iter() {
            let index = slot.index.load(Ordering::Acquire);
            if index != EMPTY_SLOT {
                let count = slot.count.load(Ordering::Relaxed);
                if count > 0 {
                    let exemplar = slot.exemplar.load_full().map(|arc| (*arc).clone());
                    out.push((index as i32, count, exemplar));
                }
            }
        }
        out
    }
}

/// A table retired by a downscale, kept alive so any in-flight `record` that
/// loaded it before the swap can still be reconciled afterwards.
///
/// `folded[i]` is the running total already folded out of slot `i` into a
/// successor table. A drain folds only the *residual* `count - folded` and
/// advances `folded` with a CAS, so every increment lands in the live table
/// exactly once even if the observe that produced it arrives long after the
/// swap (the read-then-swap straggler that the old bounded reconcile dropped).
struct RetiredTable {
    table: Arc<Table>,
    folded: Box<[AtomicU64]>,
}

/// A sparse, auto-rescaling exponential histogram.
///
/// Buckets live in a fixed-capacity lock-free table (memory tracks the number
/// of *populated* buckets, not the configured range), so it is much cheaper than
/// [`super::FixedExponentialHistogram`] for wide or unknown ranges and for many
/// instances. The observe path is lock-free: one `log2` index plus one atomic
/// increment (or a one-time CAS to claim a new bucket).
///
/// When the table fills up, the schema is **downscaled** (adjacent buckets
/// merged, halving resolution) to make room. That rebuild does not run on the
/// observe path: an observation that finds the table saturated only sets a flag
/// ([`needs_rescale`](DynamicExponentialHistogram::needs_rescale)); the actual
/// rescale happens in [`rescale_if_needed`](DynamicExponentialHistogram::rescale_if_needed),
/// which a [`Registry`](crate::Registry) calls on scrape (see
/// [`MetricTree::housekeep`](crate::MetricTree::housekeep)). The swap is RCU:
/// observers keep using the old table lock-free until a coarser one is published.
///
/// A straggling `record` can load the old table, stall, and apply its increment
/// *after* the swap. To never drop it, the retired table is kept and its residual
/// per-bucket deltas are folded into the live table on every
/// [`housekeep`](Self::housekeep), monotonically and exactly once.
/// Schema only ever decreases to [`MIN_SCHEMA`], so the retained set is bounded
/// (at most one table per schema step).
///
/// The lock-free swap backing this type is an internal implementation detail and
/// is not part of the public API.
pub struct DynamicExponentialHistogram {
    table: ArcSwap<Table>,
    zero_count: AtomicU64,
    overflow_count: AtomicU64,
    sum: AtomicF64,
    capacity: usize,
    downscale_threshold: usize,
    min_schema: i32,
    saturated: AtomicBool,
    rescaling: AtomicBool,
    /// Set permanently after the first exemplar adoption. Once exemplar windows
    /// exist, scrape-time `housekeep` reopens populated buckets every scrape.
    uses_exemplars: AtomicBool,
    /// Tables retired by past downscales, retained so straggling observers are
    /// reconciled. Mutated only off the observe hot path (downscale push, drain),
    /// under a mutex that also excludes a swap mid-drain; observe never touches it.
    retired: Mutex<Vec<RetiredTable>>,
    /// Sticky once any table has been retired: keeps `needs_housekeep` true so a
    /// registry keeps draining residual straggler deltas every scrape.
    has_retired: AtomicBool,
}

impl DynamicExponentialHistogram {
    /// Creates a histogram starting at `schema` 5 (~2.2% resolution) with a
    /// 256-bucket table, downscaling as the observed range widens.
    pub fn new() -> Self {
        DynamicExponentialHistogram::with_params(5, 256)
    }

    /// Creates a histogram with an explicit start `schema` and bucket
    /// `capacity` (rounded up to a power of two, min 8). The schema only ever
    /// decreases, down to [`MIN_SCHEMA`], as the table saturates.
    pub fn with_params(start_schema: i32, capacity: usize) -> Self {
        let start_schema = start_schema.clamp(MIN_SCHEMA, MAX_SCHEMA);
        let capacity = capacity.next_power_of_two().max(8);
        DynamicExponentialHistogram {
            table: ArcSwap::from_pointee(Table::new(capacity, start_schema)),
            zero_count: AtomicU64::new(0),
            overflow_count: AtomicU64::new(0),
            sum: AtomicF64::new(0.0),
            capacity,
            // Downscale before the open-addressed table gets too dense (probing
            // cost rises and the false-full risk grows near 100% load).
            downscale_threshold: (capacity * 3 / 4).max(1),
            min_schema: MIN_SCHEMA,
            saturated: AtomicBool::new(false),
            rescaling: AtomicBool::new(false),
            uses_exemplars: AtomicBool::new(false),
            retired: Mutex::new(Vec::new()),
            has_retired: AtomicBool::new(false),
        }
    }

    /// The current resolution schema (decreases as the range widens).
    pub fn schema(&self) -> i32 {
        self.table.load().schema
    }

    /// Records one observation. Returns the exponential bucket index (or
    /// [`ZERO_BUCKET`] for a non-positive value). Lock-free.
    pub fn observe(&self, value: f64) -> i32 {
        self.record(value, None).0
    }

    /// Records one observation and offers `exemplar` to its bucket. The exemplar
    /// is adopted as the bucket's sample if the bucket has none this scrape
    /// window (first-in-window), or if `interesting` is `true` and the standing
    /// one is not (a single upgrade per window). A non-positive value goes to
    /// the zero bucket and the exemplar is dropped; a `NaN` value is ignored
    /// entirely (no count, no sum). Both return `false`.
    ///
    /// Returns whether the offer won the bucket's exemplar eligibility
    /// transition. The stored exemplar value is best-effort under racing writers
    /// (last store wins), so the return is a sampling/retention signal rather
    /// than a guarantee that this exact `Exemplar` will be the one a later scrape
    /// emits. `interesting` and this return are intentionally inert for the
    /// plain first-in-window caller. Lock-free.
    pub fn observe_with_exemplar(&self, value: f64, exemplar: Exemplar, interesting: bool) -> bool {
        self.record(value, Some((exemplar, interesting))).1
    }

    /// Shared observe path. Returns `(bucket_index, exemplar_adopted)`.
    fn record(&self, value: f64, offer: Option<(Exemplar, bool)>) -> (i32, bool) {
        // A NaN observation is dropped before it can touch `sum` (one NaN would
        // poison `_sum` forever) or any count, and its exemplar offer is
        // discarded; non-positive finite values still fold into the zero bucket.
        if value.is_nan() {
            return (ZERO_BUCKET, false);
        }
        self.sum.add(value);
        if value <= 0.0 {
            self.zero_count.fetch_add(1, Ordering::Relaxed);
            return (ZERO_BUCKET, false);
        }
        let table = self.table.load();
        let index = index_of(value, table.schema);
        let offer_ref = offer.as_ref().map(|(ex, interesting)| (ex, *interesting));
        let (added, adopted) = table.offer(index, offer_ref);
        if added {
            if table.populated.load(Ordering::Relaxed) >= self.downscale_threshold {
                self.saturated.store(true, Ordering::Relaxed);
            }
        } else {
            self.overflow_count.fetch_add(1, Ordering::Relaxed);
            self.saturated.store(true, Ordering::Relaxed);
        }
        if adopted {
            // Enters the scrape-time exemplar-window maintenance mode. This is
            // intentionally one-way: if this scrape misses visibility of a
            // freshly-claimed slot, later scrapes still call `reopen_all` and the
            // slot self-heals instead of freezing forever.
            self.uses_exemplars.store(true, Ordering::Release);
        }
        (index, adopted)
    }

    /// Records a duration observation, converting to seconds (`f64`).
    pub fn observe_duration(&self, value: Duration) -> i32 {
        self.observe(value.as_secs_f64())
    }

    /// Whether the table has saturated and a downscale is pending. Cheap,
    /// lock-free; the maintenance path uses it to skip clean histograms.
    pub fn needs_rescale(&self) -> bool {
        self.saturated.load(Ordering::Relaxed)
    }

    /// Whether [`housekeep`](Self::housekeep) should run: a pending downscale,
    /// exemplar windows in use (reopened each scrape), or any retired table whose
    /// straggler residuals may still need folding into the live table.
    pub fn needs_housekeep(&self) -> bool {
        self.needs_rescale()
            || self.uses_exemplars.load(Ordering::Acquire)
            || self.has_retired.load(Ordering::Acquire)
    }

    /// Off-hot-path, scrape-time upkeep: performs any pending downscale, folds in
    /// any straggler increments left on retired tables, then reopens the exemplar
    /// windows so the next window samples fresh. The standing exemplar values are
    /// preserved for the scrape that follows.
    pub fn housekeep(&self) {
        self.rescale_if_needed();
        self.drain_retired();
        if self.uses_exemplars.load(Ordering::Acquire) {
            self.table.load().reopen_all();
        }
    }

    /// Performs a pending downscale, off the observe hot path.
    pub fn rescale_if_needed(&self) {
        if !self.saturated.load(Ordering::Relaxed) {
            return;
        }
        if self
            .rescaling
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.downscale();
        self.rescaling.store(false, Ordering::Release);
    }

    fn downscale(&self) {
        let old = self.table.load_full();
        let new_schema = (old.schema - 1).max(self.min_schema);
        if new_schema == old.schema {
            self.saturated.store(false, Ordering::Relaxed);
            return;
        }

        // Snapshot every slot's count once and fold it into the coarser table;
        // `folded` records exactly what was carried so a later drain only ever
        // adds the residual that arrives after this point (the straggler delta).
        // Empty slots snapshot as 0, so a slot an observer claims *after* the
        // swap still reconciles (residual = its eventual count - 0).
        let new = Arc::new(Table::new(self.capacity, new_schema));
        let mut folded = Vec::with_capacity(old.slots.len());
        for slot in old.slots.iter() {
            let index = slot.index.load(Ordering::Acquire);
            let count = slot.count.load(Ordering::Relaxed);
            folded.push(AtomicU64::new(count));
            if index != EMPTY_SLOT && count > 0 {
                self.add_or_overflow(&new, (index as i32) >> 1, count);
                if let Some(exemplar) = slot.exemplar.load_full() {
                    new.migrate_exemplar((index as i32) >> 1, exemplar);
                }
            }
        }

        // Publish the new table and retire the old one atomically with respect to
        // `drain_retired`: holding `retired` while swapping means a drain never
        // observes the live table changing out from under it mid-pass.
        {
            let mut retired = self.retired.lock();
            self.table.store(Arc::clone(&new));
            retired.push(RetiredTable {
                table: old,
                folded: folded.into_boxed_slice(),
            });
        }
        self.has_retired.store(true, Ordering::Release);

        let still_dense = new.populated.load(Ordering::Relaxed) >= self.downscale_threshold;
        self.saturated.store(
            still_dense && new_schema > self.min_schema,
            Ordering::Relaxed,
        );
    }

    /// Folds every retired table's residual per-bucket increments into the live
    /// table, exactly once each. For each slot, the residual `count - folded` is
    /// claimed with a CAS that advances `folded`, so concurrent drains and
    /// late-arriving observers can never double-count or drop an increment.
    ///
    /// A retired table at schema `s` maps bucket `i` to `i >> (s - live_schema)`
    /// in the live table -- the same halving the downscale chain applies, just
    /// composed across however many steps separate the two schemas.
    ///
    /// **Ordering:** a slot's `index` is read (Acquire) *before* the residual is
    /// claimed, and a residual is claimed only once the index is visible. A
    /// straggling observer publishes a slot in two steps -- the index CAS
    /// (AcqRel) in [`Table::slot_for`] then `count.fetch_add` (Relaxed) -- so
    /// under a weak memory model (observable on ARM, hidden by x86 TSO) the count
    /// increment can become visible to a drain while `index` still reads
    /// `EMPTY_SLOT`. If the drain claimed the residual (advanced `folded`) in
    /// that window it would have no index to fold into, and the `count > folded`
    /// guard would then make every later pass skip the slot -- losing the
    /// increment for good. Leaving an index-less slot unclaimed defers it to a
    /// later drain; because a `count` increment is only ever reached *after* the
    /// slot's index store (in the observer's program order), the index is
    /// guaranteed to become visible, so nothing is stranded and the lossless
    /// reconciliation invariant holds.
    ///
    /// The `retired` lock is held for the whole pass: it excludes a concurrent
    /// `downscale` swap (so `live` stays the live table) and serializes drains.
    /// The observe hot path never takes this lock.
    fn drain_retired(&self) {
        let retired = self.retired.lock();
        if retired.is_empty() {
            return;
        }
        let live = self.table.load();
        for entry in retired.iter() {
            let shift = (entry.table.schema - live.schema).max(0) as u32;
            for (slot, folded) in entry.table.slots.iter().zip(entry.folded.iter()) {
                loop {
                    // Read the index first: only a visible index lets us claim.
                    let index = slot.index.load(Ordering::Acquire);
                    let already = folded.load(Ordering::Acquire);
                    let current = slot.count.load(Ordering::Relaxed);
                    if current <= already {
                        break;
                    }
                    if index == EMPTY_SLOT {
                        // A residual exists but the straggler's index store is
                        // not yet visible; leave it for a later drain rather than
                        // claim a count we cannot place.
                        break;
                    }
                    if folded
                        .compare_exchange(already, current, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
                    {
                        self.add_or_overflow(&live, (index as i32) >> shift, current - already);
                        break;
                    }
                }
            }
        }
    }

    /// Adds `n` into `table`, folding into the overflow bucket if the table is
    /// full -- mirrors the observe path so a rescale never silently drops a
    /// count (the total-count invariant holds symmetrically).
    fn add_or_overflow(&self, table: &Table, index: i32, n: u64) {
        if !table.add(index, n) {
            self.overflow_count.fetch_add(n, Ordering::Relaxed);
        }
    }

    fn current_count(&self, table: &Table) -> u64 {
        self.zero_count.load(Ordering::Relaxed)
            + self.overflow_count.load(Ordering::Relaxed)
            + table.total()
    }

    /// The total number of observations recorded so far.
    pub fn count(&self) -> u64 {
        self.current_count(&self.table.load())
    }

    /// The sum of all observed values so far, in the base unit.
    pub fn sum(&self) -> f64 {
        self.sum.get()
    }

    /// Takes a sparse, non-cumulative snapshot at the current schema.
    pub fn snapshot(&self) -> ExponentialSnapshot {
        let table = self.table.load();
        let mut entries = table.populated_with_exemplars();
        entries.sort_unstable_by_key(|(index, _, _)| *index);
        ExponentialSnapshot::from_buckets(
            table.schema,
            self.zero_count.load(Ordering::Relaxed),
            self.overflow_count.load(Ordering::Relaxed),
            self.sum.get(),
            entries,
        )
    }

    /// The cumulative `le` view, for the classic OpenMetrics encoder.
    pub fn to_histogram_snapshot(&self) -> HistogramSnapshot {
        self.snapshot().to_histogram_snapshot()
    }
}

impl Default for DynamicExponentialHistogram {
    fn default() -> Self {
        DynamicExponentialHistogram::new()
    }
}

impl std::fmt::Debug for DynamicExponentialHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let table = self.table.load();
        f.debug_struct("DynamicExponentialHistogram")
            .field("schema", &table.schema)
            .field("populated", &table.populated.load(Ordering::Relaxed))
            .field("capacity", &self.capacity)
            .field("count", &self.current_count(&table))
            .field("sum", &self.sum())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exemplar(id: &str) -> Exemplar {
        Exemplar {
            labels: vec![("trace_id".to_owned(), id.to_owned())],
            value: 0.0,
            timestamp_seconds: None,
        }
    }

    fn standing_id(slot: &Slot) -> String {
        slot.exemplar.load_full().unwrap().labels[0].1.clone()
    }

    #[test]
    fn straggler_increment_on_a_retired_table_is_reconciled() {
        // Deterministic regression for the read-then-swap lost-update: a `record`
        // that loaded the table before a downscale, then applies its increment to
        // the now-retired table after the swap, must still be counted. We drive
        // the exact interleaving directly instead of racing threads: hold the old
        // table, downscale (retiring it), then increment it as a stalled observer
        // would, and require `housekeep` to fold that residual into the live table.
        let h = DynamicExponentialHistogram::with_params(6, 16);
        let mut expected = 0u64;
        let probe = 1e-5 * 1.03f64.powi(10);
        for k in 0..4000 {
            h.observe(1e-5 * 1.03f64.powi(k % 400));
            expected += 1;
            if h.needs_rescale() {
                break;
            }
        }
        assert!(h.needs_rescale(), "precondition: a downscale is pending");

        // A straggling observer's loaded view of the table, captured pre-swap.
        let old = h.table.load_full();
        let straggler_index = index_of(probe, old.schema);

        // The maintainer swaps in a coarser table and retires `old`.
        h.rescale_if_needed();
        assert!(
            !Arc::ptr_eq(&old, &h.table.load_full()),
            "downscale must swap"
        );

        // The straggler now lands its increment on the retired table -- exactly
        // the `Table::offer` the bug dropped. Before the fix this count was lost.
        assert!(
            old.add(straggler_index, 1),
            "straggler increment on retired table"
        );
        expected += 1;
        let before_drain = h.count();

        h.housekeep();
        assert_eq!(
            before_drain + 1,
            h.count(),
            "housekeep must recover the straggler's increment"
        );
        assert_eq!(h.count(), expected, "count() lost the straggler");
        assert_eq!(
            h.snapshot().count,
            expected,
            "snapshot() lost the straggler"
        );

        // Idempotent: a second drain must not double-count the same residual.
        h.housekeep();
        assert_eq!(h.count(), expected, "drain double-counted on a second pass");
        assert_eq!(h.snapshot().count, expected, "snapshot double-counted");
    }

    #[test]
    fn drain_defers_a_slot_whose_index_is_not_yet_visible() {
        // Deterministic regression for the weak-memory lost-update in
        // `drain_retired`: a straggler's `count.fetch_add` can be visible on a
        // retired slot while its `index` store is not (still EMPTY_SLOT). The
        // drain must NOT claim the residual in that window -- advancing `folded`
        // with no index to fold into would strand the increment forever -- and
        // must fold it once the index becomes visible. We forge that exact
        // interleaving directly instead of racing threads.
        let h = DynamicExponentialHistogram::with_params(6, 16);
        for k in 0..4000 {
            h.observe(1e-5 * 1.03f64.powi(k % 400));
            if h.needs_rescale() {
                break;
            }
        }
        assert!(h.needs_rescale(), "precondition: a downscale is pending");
        // Drive downscales to a stable schema so a later `housekeep` will not
        // downscale again mid-test (which would move `live` under us).
        for _ in 0..=MAX_SCHEMA {
            if !h.needs_rescale() {
                break;
            }
            h.rescale_if_needed();
        }
        assert!(!h.needs_rescale(), "schema settled");

        let baseline = h.count();

        // Forge the reorder: bump an empty retired slot's count as a straggler's
        // `fetch_add` would, but leave its index unpublished.
        forge_straggler_count(&h);

        // Index not visible yet -> the drain must leave the residual unclaimed,
        // so the count is unchanged (the bug would have advanced `folded` here).
        h.housekeep();
        assert_eq!(
            h.count(),
            baseline,
            "drain must not claim a slot whose index is not yet visible"
        );

        // Publish the index; the next drain folds the deferred residual once.
        publish_forged_index(&h);
        h.housekeep();
        assert_eq!(
            h.count(),
            baseline + 1,
            "the deferred residual is folded once the index appears"
        );
        h.housekeep();
        assert_eq!(
            h.count(),
            baseline + 1,
            "a later drain must not double-count the deferred residual"
        );
    }

    /// Bumps the count of one empty slot in the oldest retired table without
    /// publishing its index -- the observable half of a straggler mid-flight.
    fn forge_straggler_count(h: &DynamicExponentialHistogram) {
        let retired = h.retired.lock();
        let entry = retired.first().expect("a table was retired");
        let slot = entry
            .table
            .slots
            .iter()
            .find(|slot| slot.index.load(Ordering::Acquire) == EMPTY_SLOT)
            .expect("a 16-wide retired table has empty slots");
        slot.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Publishes the index for the slot forged by [`forge_straggler_count`],
    /// completing the deferred straggler so a later drain can fold it.
    fn publish_forged_index(h: &DynamicExponentialHistogram) {
        let retired = h.retired.lock();
        let entry = retired.first().expect("a table was retired");
        let slot = entry
            .table
            .slots
            .iter()
            .find(|slot| {
                slot.count.load(Ordering::Relaxed) > 0
                    && slot.index.load(Ordering::Acquire) == EMPTY_SLOT
            })
            .expect("the forged straggler slot");
        slot.index.store(0, Ordering::Release);
    }

    #[test]
    fn slot_window_state_adopts_once_upgrades_once_and_reopens() {
        let slot = Slot::empty();

        assert!(slot.offer_exemplar(&exemplar("first"), false));
        assert_eq!(standing_id(&slot), "first");

        assert!(!slot.offer_exemplar(&exemplar("second"), false));
        assert_eq!(standing_id(&slot), "first");

        assert!(slot.offer_exemplar(&exemplar("error"), true));
        assert_eq!(standing_id(&slot), "error");

        assert!(!slot.offer_exemplar(&exemplar("error2"), true));
        assert!(!slot.offer_exemplar(&exemplar("boring"), false));
        assert_eq!(standing_id(&slot), "error");

        slot.reopen();
        assert!(slot.offer_exemplar(&exemplar("next-window"), false));
        assert_eq!(standing_id(&slot), "next-window");
    }
}
