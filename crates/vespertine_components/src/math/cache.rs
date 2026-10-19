//! The process-wide bounded formula cache (CMP-012 R10, R12).
//!
//! Keyed by (face, display flag, source); bounded by an entry count and by retained bytes (an
//! entry weighs the source length plus its items times the item size); least recently used
//! entries go first. Errors are cached like formulas as an empty result. The lock is held only
//! to look up and to insert, never through a layout; when two threads miss on the same key and
//! both lay it out, the first insertion wins and both callers receive its result.

use super::faces::{FACE_COUNT, MathFace};
use super::formula::Formula;
use super::layout::{self, MAX_SOURCE_BYTES};
use super::parse::MathError;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

/// Entry bound of the process-wide cache.
pub const MAX_ENTRIES: usize = 4096;
/// Byte bound of the process-wide cache: 8 MiB.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// Stack of the layout thread; the nesting limit of 64 needs far less, a 512 KiB secondary
/// thread would be too little for the deepest accepted formulas.
const LAYOUT_STACK_BYTES: usize = 8 * 1024 * 1024;

struct Slot {
    result: Option<Arc<Formula>>,
    weight: usize,
    tick: u64,
}

#[derive(Default)]
struct Lru {
    /// One map per (face, display flag).
    maps: Vec<HashMap<Arc<str>, Slot>>,
    /// Recency order: oldest tick first.
    order: BTreeMap<u64, (usize, Arc<str>)>,
    tick: u64,
    bytes: usize,
    len: usize,
}

impl Lru {
    fn new() -> Lru {
        Lru {
            maps: (0..FACE_COUNT * 2).map(|_| HashMap::new()).collect(),
            ..Lru::default()
        }
    }

    fn touch(&mut self, map: usize, source: &str) -> Option<Option<Arc<Formula>>> {
        self.tick += 1;
        let tick = self.tick;
        let slot = self.maps[map].get_mut(source)?;
        let previous = std::mem::replace(&mut slot.tick, tick);
        let result = slot.result.clone();
        if let Some(key) = self.order.remove(&previous) {
            self.order.insert(tick, key);
        }
        Some(result)
    }

    fn insert(&mut self, map: usize, source: &str, result: Option<Arc<Formula>>) {
        self.tick += 1;
        let weight = result
            .as_ref()
            .map_or(source.len(), |formula| formula.weight(source.len()));
        let key: Arc<str> = Arc::from(source);
        self.order.insert(self.tick, (map, key.clone()));
        self.maps[map].insert(
            key,
            Slot {
                result,
                weight,
                tick: self.tick,
            },
        );
        self.bytes += weight;
        self.len += 1;
    }

    fn evict_oldest(&mut self) -> bool {
        let Some((_, (map, key))) = self.order.pop_first() else {
            return false;
        };
        if let Some(slot) = self.maps[map].remove(&key) {
            self.bytes -= slot.weight;
            self.len -= 1;
        }
        true
    }
}

/// A bounded LRU cache of laid-out formulas.
pub struct FormulaCache {
    inner: Mutex<Lru>,
    max_entries: usize,
    max_bytes: usize,
    layouts: AtomicUsize,
}

static GLOBAL: LazyLock<FormulaCache> =
    LazyLock::new(|| FormulaCache::with_limits(MAX_ENTRIES, MAX_BYTES));

impl FormulaCache {
    fn lock(&self) -> MutexGuard<'_, Lru> {
        // A poisoned lock only means another thread panicked outside the cache's invariants.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A cache with the given entry and byte bounds (the process-wide one uses 4096 and 8 MiB).
    pub fn with_limits(max_entries: usize, max_bytes: usize) -> FormulaCache {
        FormulaCache {
            inner: Mutex::new(Lru::new()),
            max_entries,
            max_bytes,
            layouts: AtomicUsize::new(0),
        }
    }

    /// The process-wide cache.
    pub fn global() -> &'static FormulaCache {
        &GLOBAL
    }

    /// The formula of `source` for `face`, laid out on a thread with a large stack on a miss;
    /// `None` for every error (cached) and for sources over 16 KiB (refused before parsing and
    /// never cached).
    pub fn lookup(&self, source: &str, display: bool, face: MathFace) -> Option<Arc<Formula>> {
        self.lookup_with(source, display, face, || {
            layout_on_big_stack(source, display, face)
        })
    }

    /// As [`FormulaCache::lookup`] with an explicit layout function (the seam that lets tests
    /// observe and order layouts). The lock is not held while `layout` runs.
    pub fn lookup_with(
        &self,
        source: &str,
        display: bool,
        face: MathFace,
        layout: impl FnOnce() -> Result<Formula, MathError>,
    ) -> Option<Arc<Formula>> {
        if source.len() > MAX_SOURCE_BYTES {
            return None;
        }
        let map = face.index() * 2 + usize::from(display);
        if let Some(hit) = self.lock().touch(map, source) {
            return hit;
        }
        self.layouts.fetch_add(1, Ordering::SeqCst);
        let result = layout().ok().map(Arc::new);
        let mut inner = self.lock();
        // Another thread may have inserted the same key while this one laid out: the first
        // insertion wins and both callers get its result.
        if let Some(existing) = inner.touch(map, source) {
            return existing;
        }
        inner.insert(map, source, result.clone());
        while (inner.len > self.max_entries || inner.bytes > self.max_bytes) && inner.evict_oldest()
        {
        }
        result
    }

    /// Number of layouts performed through this cache so far.
    pub fn layout_count(&self) -> usize {
        self.layouts.load(Ordering::SeqCst)
    }

    /// Retained bytes: the sum of the weights of the entries.
    pub fn retained_bytes(&self) -> usize {
        self.lock().bytes
    }

    /// Number of entries (formulas and cached errors).
    pub fn len(&self) -> usize {
        self.lock().len
    }

    /// Whether the cache holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The weight of every entry, in no particular order.
    pub fn weights(&self) -> Vec<usize> {
        self.lock()
            .maps
            .iter()
            .flat_map(|map| map.values().map(|slot| slot.weight))
            .collect()
    }

    /// Whether (source, display, face) is cached (without marking it recently used).
    pub fn contains(&self, source: &str, display: bool, face: MathFace) -> bool {
        self.lock().maps[face.index() * 2 + usize::from(display)].contains_key(source)
    }

    /// Whether the cached entry is an error (`None` when absent). Does not mark it used.
    pub fn is_cached_error(&self, source: &str, display: bool, face: MathFace) -> Option<bool> {
        self.lock().maps[face.index() * 2 + usize::from(display)]
            .get(source)
            .map(|slot| slot.result.is_none())
    }
}

/// The engine's cached entry point: the formula of `source` in the process-wide cache.
pub fn lookup(source: &str, display: bool, face: MathFace) -> Option<Arc<Formula>> {
    FormulaCache::global().lookup(source, display, face)
}

fn layout_on_big_stack(source: &str, display: bool, face: MathFace) -> Result<Formula, MathError> {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("math-layout".into())
            .stack_size(LAYOUT_STACK_BYTES)
            .spawn_scoped(scope, || layout::layout(source, display, face))
            .map_err(|error| MathError::Syntax(format!("layout thread: {error}")))?
            .join()
            .map_err(|_| MathError::Syntax("layout panicked".into()))?
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::formula::DisplayItem;

    fn formula_with(items: usize) -> Formula {
        Formula::from_items(
            vec![
                DisplayItem::Rule {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 0.1
                };
                items
            ],
            1.0,
            1.0,
            0.0,
        )
        .unwrap()
    }

    #[test]
    fn items_weigh_their_size_and_the_source_length() {
        let formula = formula_with(3);
        assert_eq!(
            formula.weight(10),
            10 + 3 * std::mem::size_of::<DisplayItem>()
        );
    }

    #[test]
    fn the_least_recently_used_entry_is_evicted_by_count() {
        let cache = FormulaCache::with_limits(2, usize::MAX);
        let face = MathFace::DEFAULT;
        let make = |n: usize| move || Ok(formula_with(n));
        cache.lookup_with("a", false, face, make(1));
        cache.lookup_with("b", false, face, make(1));
        // Touching the first entry makes the second the victim.
        cache.lookup_with("a", false, face, make(1));
        cache.lookup_with("c", false, face, make(1));
        assert!(cache.contains("a", false, face));
        assert!(!cache.contains("b", false, face));
        assert!(cache.contains("c", false, face));
        assert_eq!(cache.layout_count(), 3);
    }
}
