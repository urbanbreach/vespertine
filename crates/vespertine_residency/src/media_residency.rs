//! Media residency budgets (PRF-017 R6, R7, R8).
//!
//! Media that exist only while their owning content is expanded or visible are resident only
//! while requested and for a grace period afterwards. A [`ResidencyTracker`] accounts the decoded
//! bytes of every resident item against a byte budget and evicts least-recently-used items that
//! are not currently requested; items that are requested are never evicted. Evicted items are
//! queued for the owner to release (the image-atlas release of an evicted image is
//! `ImageResidency`'s job). [`ClassedResidency`] gives every attachment class its own tracker so
//! that one class never evicts another's items.
//!
//! Time is an explicit `Instant` argument, so the tracker is deterministic; a driver schedules a
//! timer for [`ResidencyTracker::next_deadline`] and calls [`ResidencyTracker::tick`] when it
//! fires.

use std::{
    collections::{BTreeMap, HashMap},
    hash::Hash,
    time::{Duration, Instant},
};

/// Budget and grace period of one residency class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidencyConfig {
    /// Most decoded bytes resident at once (requested items may exceed it, see
    /// [`ResidencyTracker::insert`]).
    pub budget_bytes: usize,
    /// How long a released item stays resident for a new request to reuse it.
    pub grace: Duration,
}

impl ResidencyConfig {
    /// Expanded-content previews: 64 MiB, 600 ms collapse grace.
    pub const PREVIEWS: Self = Self {
        budget_bytes: 64 * 1024 * 1024,
        grace: Duration::from_millis(600),
    };
    /// One attachment cache class: 64 MiB, 600 ms collapse grace.
    pub const ATTACHMENT_CLASS: Self = Self::PREVIEWS;
}

/// An item taken out of residency; the owner releases whatever it holds (atlas entries,
/// decoded data).
#[derive(Debug)]
pub struct Evicted<K, V> {
    /// The item's identity.
    pub key: K,
    /// The resident payload.
    pub value: V,
    /// The decoded bytes the item was accounted with.
    pub bytes: usize,
}

/// Counters of one tracker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResidencyStats {
    /// Items now resident.
    pub items: usize,
    /// Decoded bytes now resident.
    pub bytes: usize,
    /// Items now requested.
    pub requested: usize,
    /// Items inserted (decoded and uploaded).
    pub inserts: u64,
    /// Requests served by a resident item without decoding.
    pub reuses: u64,
    /// Items evicted for any reason.
    pub evictions: u64,
    /// Items evicted because their grace period elapsed.
    pub expirations: u64,
    /// Items evicted to bring the resident bytes within the budget.
    pub budget_evictions: u64,
}

struct Item<V> {
    value: V,
    bytes: usize,
    requested: bool,
    /// When the item was released; its grace period runs from here.
    idle_since: Option<Instant>,
    tick: u64,
}

/// Residency accounting for one class of media.
pub struct ResidencyTracker<K, V> {
    config: ResidencyConfig,
    items: HashMap<K, Item<V>>,
    /// Recency order: the smallest tick is the least recently used item.
    order: BTreeMap<u64, K>,
    tick: u64,
    bytes: usize,
    evicted: Vec<Evicted<K, V>>,
    stats: ResidencyStats,
}

impl<K: Clone + Eq + Hash, V> ResidencyTracker<K, V> {
    /// A tracker with `config`.
    pub fn new(config: ResidencyConfig) -> Self {
        Self {
            config,
            items: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            bytes: 0,
            evicted: Vec::new(),
            stats: ResidencyStats::default(),
        }
    }

    /// The configuration.
    pub fn config(&self) -> ResidencyConfig {
        self.config
    }

    /// Decoded bytes resident.
    pub fn resident_bytes(&self) -> usize {
        self.bytes
    }

    /// Items resident.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing is resident.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The counters.
    pub fn stats(&self) -> ResidencyStats {
        ResidencyStats {
            items: self.items.len(),
            bytes: self.bytes,
            requested: self.items.values().filter(|item| item.requested).count(),
            ..self.stats
        }
    }

    /// Whether `key` is resident (including one whose grace period has elapsed but that no tick
    /// has released yet).
    pub fn contains(&self, key: &K) -> bool {
        self.items.contains_key(key)
    }

    /// Whether `key` is resident and currently requested.
    pub fn is_requested(&self, key: &K) -> bool {
        self.items.get(key).is_some_and(|item| item.requested)
    }

    /// The payload of a resident item, without requesting it.
    pub fn peek(&self, key: &K) -> Option<&V> {
        self.items.get(key).map(|item| &item.value)
    }

    fn expired(&self, item: &Item<V>, now: Instant) -> bool {
        item.idle_since
            .is_some_and(|since| now.saturating_duration_since(since) >= self.config.grace)
    }

    /// Requests `key` at `now`. A resident item is marked requested, its pending eviction is
    /// cancelled and it is returned without any decoding or uploading; requesting an item that
    /// is already requested is idempotent (a host may request every frame). An item whose grace
    /// period had elapsed is released first and reported as absent: the caller re-creates it
    /// and calls [`Self::insert`].
    pub fn request(&mut self, key: &K, now: Instant) -> Option<&V> {
        let item = self.items.get(key)?;
        if self.expired(item, now) {
            self.stats.expirations += 1;
            self.evict(key);
            return None;
        }
        let item = self.items.get_mut(key).expect("checked above");
        if !item.requested {
            self.stats.reuses += 1;
        }
        item.requested = true;
        item.idle_since = None;
        self.order.remove(&item.tick);
        self.tick += 1;
        item.tick = self.tick;
        self.order.insert(self.tick, key.clone());
        Some(&item.value)
    }

    /// Makes a freshly created item resident and requested, accounted with `bytes` decoded
    /// bytes, and enforces the budget: least-recently-used items that are not requested are
    /// evicted while the resident bytes exceed the budget. A requested item is never evicted,
    /// so a request set larger than the budget stays resident until it is released.
    /// An existing item of the same key is replaced and queued for release.
    pub fn insert(&mut self, key: K, value: V, bytes: usize, now: Instant) {
        self.evict(&key);
        self.tick += 1;
        self.order.insert(self.tick, key.clone());
        self.items.insert(
            key,
            Item {
                value,
                bytes,
                requested: true,
                idle_since: None,
                tick: self.tick,
            },
        );
        self.bytes += bytes;
        self.stats.inserts += 1;
        self.enforce(now);
    }

    /// Releases `key` at `now`: the owning content collapsed or scrolled out. The item stays
    /// resident for the grace period; the returned deadline is when a timer must call
    /// [`Self::tick`]. Returns `None` for an item that is not resident or not requested.
    pub fn release(&mut self, key: &K, now: Instant) -> Option<Instant> {
        let item = self.items.get_mut(key)?;
        if !item.requested {
            return None;
        }
        item.requested = false;
        item.idle_since = Some(now);
        let deadline = now + self.config.grace;
        self.enforce(now);
        Some(deadline)
    }

    /// Removes `key` immediately (its content was removed), queueing it for release.
    pub fn remove(&mut self, key: &K) {
        self.evict(key);
    }

    /// Releases every item whose grace period has elapsed and then evicts least-recently-used
    /// unrequested items while the resident bytes exceed the budget. Returns the number of
    /// items evicted. Call when the timer for [`Self::next_deadline`] fires; the release of an
    /// expired item does not depend on any other event.
    pub fn tick(&mut self, now: Instant) -> usize {
        let before = self.stats.evictions;
        self.enforce(now);
        (self.stats.evictions - before) as usize
    }

    /// The earliest moment an idle item's grace period ends.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.items
            .values()
            .filter_map(|item| item.idle_since)
            .min()
            .map(|since| since + self.config.grace)
    }

    /// Takes the items evicted since the last call; the owner releases them.
    pub fn take_evicted(&mut self) -> Vec<Evicted<K, V>> {
        std::mem::take(&mut self.evicted)
    }

    fn enforce(&mut self, now: Instant) {
        let expired: Vec<K> = self
            .items
            .iter()
            .filter(|(_, item)| self.expired(item, now))
            .map(|(key, _)| key.clone())
            .collect();
        for key in expired {
            self.stats.expirations += 1;
            self.evict(&key);
        }
        while self.bytes > self.config.budget_bytes {
            let victim = self
                .order
                .values()
                .find(|key| !self.items[*key].requested)
                .cloned();
            let Some(victim) = victim else {
                break;
            };
            self.stats.budget_evictions += 1;
            self.evict(&victim);
        }
    }

    fn evict(&mut self, key: &K) {
        let Some(item) = self.items.remove(key) else {
            return;
        };
        self.order.remove(&item.tick);
        self.bytes -= item.bytes;
        self.stats.evictions += 1;
        self.evicted.push(Evicted {
            key: key.clone(),
            value: item.value,
            bytes: item.bytes,
        });
    }
}

/// One [`ResidencyTracker`] per class (for example per attachment kind), each with its own
/// budget: an item of one class is never evicted for the bytes of another.
pub struct ClassedResidency<C, K, V> {
    default: ResidencyConfig,
    classes: HashMap<C, ResidencyTracker<K, V>>,
}

impl<C: Clone + Eq + Hash, K: Clone + Eq + Hash, V> ClassedResidency<C, K, V> {
    /// A set whose classes use `default` unless configured otherwise.
    pub fn new(default: ResidencyConfig) -> Self {
        Self {
            default,
            classes: HashMap::new(),
        }
    }

    /// Gives `class` its own configuration; a class that already exists keeps its own.
    pub fn configure(&mut self, class: C, config: ResidencyConfig) {
        self.classes
            .entry(class)
            .or_insert_with(|| ResidencyTracker::new(config));
    }

    /// The tracker of `class`, created with the default configuration on first use.
    pub fn class_mut(&mut self, class: &C) -> &mut ResidencyTracker<K, V> {
        let default = self.default;
        self.classes
            .entry(class.clone())
            .or_insert_with(|| ResidencyTracker::new(default))
    }

    /// The tracker of `class`, if it exists.
    pub fn class(&self, class: &C) -> Option<&ResidencyTracker<K, V>> {
        self.classes.get(class)
    }

    /// Decoded bytes resident in every class.
    pub fn resident_bytes(&self) -> usize {
        self.classes
            .values()
            .map(|tracker| tracker.resident_bytes())
            .sum()
    }

    /// Ticks every class; returns the number of items evicted.
    pub fn tick(&mut self, now: Instant) -> usize {
        self.classes
            .values_mut()
            .map(|tracker| tracker.tick(now))
            .sum()
    }

    /// The earliest grace deadline of any class.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.classes
            .values()
            .filter_map(|tracker| tracker.next_deadline())
            .min()
    }

    /// Takes the items every class evicted since the last call.
    pub fn take_evicted(&mut self) -> Vec<Evicted<K, V>> {
        self.classes
            .values_mut()
            .flat_map(|tracker| tracker.take_evicted())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: usize = 1024 * 1024;
    const GRACE: Duration = Duration::from_millis(600);

    fn tracker(budget: usize) -> ResidencyTracker<u32, &'static str> {
        ResidencyTracker::new(ResidencyConfig {
            budget_bytes: budget,
            grace: GRACE,
        })
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn reopening_within_the_grace_period_reuses_the_item_and_cancels_the_eviction() {
        let t0 = Instant::now();
        let mut tracker = tracker(10 * MIB);
        tracker.insert(1, "image", MIB, t0);
        let deadline = tracker.release(&1, at(t0, 100)).unwrap();
        assert_eq!(deadline, at(t0, 700));
        assert_eq!(tracker.next_deadline(), Some(at(t0, 700)));

        assert_eq!(tracker.request(&1, at(t0, 650)), Some(&"image"));
        assert!(tracker.is_requested(&1));
        assert_eq!(
            tracker.next_deadline(),
            None,
            "the pending eviction was cancelled"
        );
        // The old deadline passes with nothing to release.
        assert_eq!(tracker.tick(at(t0, 800)), 0);
        assert!(tracker.contains(&1));
        let stats = tracker.stats();
        assert_eq!((stats.inserts, stats.reuses, stats.evictions), (1, 1, 0));
        assert!(tracker.take_evicted().is_empty());
    }

    #[test]
    fn reopening_after_the_grace_period_recreates_the_item() {
        let t0 = Instant::now();
        let mut tracker = tracker(10 * MIB);
        tracker.insert(1, "first", MIB, t0);
        tracker.release(&1, t0);
        // Just before the deadline the item is still reusable ...
        assert!(tracker.request(&1, at(t0, 599)).is_some());
        tracker.release(&1, at(t0, 599));
        // ... a tick at the deadline releases it without any other event ...
        assert_eq!(tracker.tick(at(t0, 599 + 600)), 1);
        let evicted = tracker.take_evicted();
        assert_eq!(evicted.len(), 1);
        assert_eq!(
            (evicted[0].key, evicted[0].value, evicted[0].bytes),
            (1, "first", MIB)
        );
        assert_eq!(tracker.resident_bytes(), 0);
        // ... and a request after it is a miss that the caller re-creates.
        assert!(tracker.request(&1, at(t0, 1500)).is_none());
        tracker.insert(1, "second", MIB, at(t0, 1500));
        assert_eq!(tracker.peek(&1), Some(&"second"));
        assert_eq!(tracker.stats().inserts, 2);
    }

    #[test]
    fn a_request_after_the_deadline_without_a_tick_still_misses() {
        let t0 = Instant::now();
        let mut tracker = tracker(10 * MIB);
        tracker.insert(1, "a", MIB, t0);
        tracker.release(&1, t0);
        assert!(tracker.request(&1, at(t0, 600)).is_none());
        assert_eq!(tracker.take_evicted().len(), 1);
        assert_eq!(tracker.stats().expirations, 1);
    }

    #[test]
    fn budget_evicts_least_recently_used_unrequested_items_and_never_requested_ones() {
        let t0 = Instant::now();
        let mut tracker = tracker(3 * MIB);
        for key in 0..3 {
            tracker.insert(key, "x", MIB, t0);
        }
        // All three are requested: a fourth exceeds the budget but nothing can be evicted.
        tracker.insert(3, "x", MIB, t0);
        assert_eq!((tracker.len(), tracker.resident_bytes()), (4, 4 * MIB));
        assert!(tracker.take_evicted().is_empty());

        // Releasing an item makes it evictable and, over budget, evicts it at once.
        tracker.release(&2, at(t0, 10));
        assert_eq!(
            tracker.resident_bytes(),
            3 * MIB,
            "release enforces the budget"
        );
        let evicted: Vec<u32> = tracker.take_evicted().iter().map(|e| e.key).collect();
        assert_eq!(evicted, vec![2]);
        tracker.release(&1, at(t0, 20));
        assert!(
            tracker.take_evicted().is_empty(),
            "within the budget a released item waits"
        );
        assert!(tracker.contains(&0) && tracker.contains(&1) && tracker.contains(&3));

        // Requests refresh recency: request 1 again, release 0 and 3, then insert one more.
        tracker.request(&1, at(t0, 30));
        tracker.release(&0, at(t0, 31));
        tracker.release(&3, at(t0, 32));
        tracker.insert(4, "x", MIB, at(t0, 33));
        let evicted: Vec<u32> = tracker.take_evicted().iter().map(|e| e.key).collect();
        assert_eq!(evicted, vec![0], "0 was touched before 3");
        assert!(tracker.is_requested(&1) && tracker.is_requested(&4));
        assert!(tracker.resident_bytes() <= 3 * MIB);
    }

    #[test]
    fn an_over_budget_request_set_shrinks_as_soon_as_items_are_released() {
        let t0 = Instant::now();
        let mut tracker = tracker(2 * MIB);
        tracker.insert(1, "big", 3 * MIB, t0);
        assert_eq!(
            tracker.resident_bytes(),
            3 * MIB,
            "a requested item is never evicted"
        );
        tracker.release(&1, at(t0, 5));
        assert_eq!(tracker.resident_bytes(), 0);
        assert_eq!(tracker.take_evicted().len(), 1);
    }

    #[test]
    fn replacing_an_item_queues_the_old_payload_for_release() {
        let t0 = Instant::now();
        let mut tracker = tracker(10 * MIB);
        tracker.insert(1, "old", MIB, t0);
        tracker.insert(1, "new", 2 * MIB, t0);
        assert_eq!(tracker.resident_bytes(), 2 * MIB);
        let evicted = tracker.take_evicted();
        assert_eq!((evicted[0].value, evicted[0].bytes), ("old", MIB));
        tracker.remove(&1);
        assert_eq!(tracker.resident_bytes(), 0);
        assert_eq!(tracker.take_evicted().len(), 1);
    }

    #[test]
    fn releasing_twice_or_an_unknown_item_returns_no_deadline() {
        let t0 = Instant::now();
        let mut tracker = tracker(MIB);
        assert_eq!(tracker.release(&9, t0), None);
        tracker.insert(1, "a", 1, t0);
        assert!(tracker.release(&1, t0).is_some());
        assert_eq!(
            tracker.release(&1, at(t0, 50)),
            None,
            "the grace period runs from the first release"
        );
        assert_eq!(tracker.next_deadline(), Some(at(t0, 600)));
    }

    #[test]
    fn classes_have_independent_budgets_and_never_evict_each_other() {
        let t0 = Instant::now();
        let mut classes: ClassedResidency<&str, u32, &str> =
            ClassedResidency::new(ResidencyConfig {
                budget_bytes: 2 * MIB,
                grace: GRACE,
            });
        classes.configure(
            "large",
            ResidencyConfig {
                budget_bytes: 8 * MIB,
                grace: GRACE,
            },
        );
        classes.class_mut(&"pdf").insert(1, "pdf-1", 2 * MIB, t0);
        classes.class_mut(&"pdf").release(&1, t0);
        // Filling the "image" class far past its own budget leaves the pdf item alone.
        for key in 0..6 {
            let images = classes.class_mut(&"image");
            images.insert(key, "image", MIB, t0);
            images.release(&key, t0);
        }
        assert!(classes.class(&"pdf").unwrap().contains(&1));
        assert!(classes.class(&"image").unwrap().resident_bytes() <= 2 * MIB);
        assert_eq!(
            classes.class(&"large").map(|c| c.config().budget_bytes),
            Some(8 * MIB)
        );
        // Every class releases its expired items on the same tick.
        assert_eq!(classes.next_deadline(), Some(at(t0, 600)));
        let evicted = classes.tick(at(t0, 600));
        assert!(evicted >= 3);
        assert_eq!(classes.resident_bytes(), 0);
        assert!(!classes.take_evicted().is_empty());
    }

    #[test]
    fn default_budgets_match_the_spec() {
        assert_eq!(ResidencyConfig::PREVIEWS.budget_bytes, 64 * MIB);
        assert_eq!(ResidencyConfig::PREVIEWS.grace, Duration::from_millis(600));
        assert_eq!(ResidencyConfig::ATTACHMENT_CLASS.budget_bytes, 64 * MIB);
    }
}
