//! Bounded recent shaping store (PRF-012).
//!
//! The state belongs to one [`TextSystem`](crate::TextSystem) and is shared by every window
//! text system built on it, so it is guarded by a lock. The lock is never held while the
//! platform shapes text.

use super::{FontRun, LineLayout};
use crate::{Pixels, PlatformTextSystem, SharedString};
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::{
    hash::{Hash, Hasher},
    sync::atomic::{AtomicU64, Ordering},
};
use vespertine_collections::{FxHashMap, FxHasher};

/// Glyph units one generation of recent shapes may hold before the generations rotate.
pub(crate) const RECENT_GENERATION_GLYPH_UNITS: usize = 16_384;

/// Counters describing shaping work. Compiled always (relaxed atomics), read only by tests and
/// diagnostics.
#[derive(Default)]
pub(crate) struct ShapeCounters {
    pub(crate) platform_line_shapes: AtomicU64,
    pub(crate) recent_hits: AtomicU64,
    pub(crate) recent_rotations: AtomicU64,
    pub(crate) lock_acquisitions: AtomicU64,
    /// Lines that missed the frame cache and the recent-shape store and were shaped by the
    /// platform. Feeds `FrameStats::lines_shaped` (DEV-003).
    pub(crate) shaped_lines: AtomicU64,
    /// Wall time spent shaping those lines, recorded only while shaping timing is enabled.
    pub(crate) shaping_nanos: AtomicU64,
}

impl ShapeCounters {
    #[inline]
    pub(crate) fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// A snapshot of the shaping counters of one text system.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShapeStats {
    /// Full lines handed to the platform text system.
    pub platform_line_shapes: u64,
    /// Lines served by the recent-shape store.
    pub recent_hits: u64,
    /// Generation rotations of the recent-shape store.
    pub recent_rotations: u64,
    /// Acquisitions of the recent-shape lock.
    pub lock_acquisitions: u64,
    /// Glyph units currently retained by both recent generations.
    pub retained_glyph_units: usize,
}

#[derive(PartialEq, Eq)]
struct RecentKey {
    text: SharedString,
    size_bits: u32,
    runs: SmallVec<[FontRun; 2]>,
}

impl RecentKey {
    fn matches(&self, text: &str, size_bits: u32, runs: &[FontRun]) -> bool {
        self.size_bits == size_bits && self.runs.as_slice() == runs && self.text.as_str() == text
    }
}

struct RecentEntry {
    key: RecentKey,
    layout: LineLayout,
    units: usize,
}

#[derive(Default)]
struct Generation {
    buckets: FxHashMap<u64, SmallVec<[RecentEntry; 1]>>,
    units: usize,
}

fn recent_hash(text: &str, size_bits: u32, runs: &[FontRun]) -> u64 {
    let mut hasher = FxHasher::default();
    text.hash(&mut hasher);
    size_bits.hash(&mut hasher);
    runs.hash(&mut hasher);
    hasher.finish()
}

fn glyph_units(layout: &LineLayout) -> usize {
    layout
        .runs
        .iter()
        .map(|run| run.glyphs.len())
        .sum::<usize>()
        .max(1)
}

#[derive(Default)]
struct RecentShapes {
    font_generation: usize,
    current: Generation,
    old: Generation,
    /// Test seam: forces every key into the same bucket.
    #[cfg(test)]
    constant_hash: Option<u64>,
}

impl RecentShapes {
    fn hash(&self, text: &str, size_bits: u32, runs: &[FontRun]) -> u64 {
        #[cfg(test)]
        if let Some(hash) = self.constant_hash {
            return hash;
        }
        recent_hash(text, size_bits, runs)
    }

    /// Aligns the store with the font generation the caller observed. Returns `false` when the
    /// caller observed a stale generation and must neither read nor write the store.
    fn sync(&mut self, font_generation: usize) -> bool {
        if font_generation > self.font_generation {
            self.current = Generation::default();
            self.old = Generation::default();
            self.font_generation = font_generation;
            true
        } else {
            font_generation == self.font_generation
        }
    }

    fn insert_entry(&mut self, entry: RecentEntry, hash: u64, counters: &ShapeCounters) {
        if self.current.units >= RECENT_GENERATION_GLYPH_UNITS {
            self.old = std::mem::take(&mut self.current);
            ShapeCounters::bump(&counters.recent_rotations);
        }
        self.current.units += entry.units;
        self.current.buckets.entry(hash).or_default().push(entry);
    }

    fn lookup(
        &mut self,
        text: &str,
        size_bits: u32,
        runs: &[FontRun],
        counters: &ShapeCounters,
    ) -> Option<LineLayout> {
        let hash = self.hash(text, size_bits, runs);
        if let Some(bucket) = self.current.buckets.get(&hash)
            && let Some(entry) = bucket
                .iter()
                .find(|entry| entry.key.matches(text, size_bits, runs))
        {
            return Some(entry.layout.clone());
        }

        let bucket = self.old.buckets.get_mut(&hash)?;
        let position = bucket
            .iter()
            .position(|entry| entry.key.matches(text, size_bits, runs))?;
        let entry = bucket.swap_remove(position);
        if bucket.is_empty() {
            self.old.buckets.remove(&hash);
        }
        self.old.units = self.old.units.saturating_sub(entry.units);
        let layout = entry.layout.clone();
        self.insert_entry(entry, hash, counters);
        Some(layout)
    }

    fn insert(
        &mut self,
        text: &SharedString,
        size_bits: u32,
        runs: &[FontRun],
        layout: &LineLayout,
        counters: &ShapeCounters,
    ) {
        let hash = self.hash(text, size_bits, runs);
        if let Some(bucket) = self.current.buckets.get(&hash)
            && bucket
                .iter()
                .any(|entry| entry.key.matches(text, size_bits, runs))
        {
            return;
        }
        let entry = RecentEntry {
            key: RecentKey {
                text: text.clone(),
                size_bits,
                runs: SmallVec::from(runs),
            },
            units: glyph_units(layout),
            layout: layout.clone(),
        };
        self.insert_entry(entry, hash, counters);
    }
}

/// Shaping state shared by every window text system of one text system.
#[derive(Default)]
pub(crate) struct ShapeCaches {
    recent: Mutex<RecentShapes>,
    pub(crate) counters: ShapeCounters,
    timing_enabled: std::sync::atomic::AtomicBool,
}

impl ShapeCaches {
    /// Enables or disables timing of shaped lines (off by default).
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn set_timing_enabled(&self, enabled: bool) {
        self.timing_enabled.store(enabled, Ordering::Relaxed);
    }

    /// Lines shaped and nanoseconds spent shaping them since this text system was created.
    pub(crate) fn shaping_totals(&self) -> (u64, u64) {
        (
            self.counters.shaped_lines.load(Ordering::Relaxed),
            self.counters.shaping_nanos.load(Ordering::Relaxed),
        )
    }

    /// Counts one shaped line and, when timing is enabled, the time `shape` took.
    pub(crate) fn record_shaped<R>(&self, shape: impl FnOnce() -> R) -> R {
        ShapeCounters::bump(&self.counters.shaped_lines);
        if !self.timing_enabled.load(Ordering::Relaxed) {
            return shape();
        }
        let start = std::time::Instant::now();
        let result = shape();
        let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.counters
            .shaping_nanos
            .fetch_add(nanos, Ordering::Relaxed);
        result
    }

    /// Looks a line up in the recent-shape store. `font_generation` is the generation the caller
    /// observed before looking.
    pub(crate) fn recent_lookup(
        &self,
        font_generation: usize,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> Option<LineLayout> {
        let size_bits = font_size.0.to_bits();
        let mut recent = self.recent.lock();
        ShapeCounters::bump(&self.counters.lock_acquisitions);
        if !recent.sync(font_generation) {
            return None;
        }
        let layout = recent.lookup(text, size_bits, runs, &self.counters)?;
        ShapeCounters::bump(&self.counters.recent_hits);
        Some(layout)
    }

    /// Stores a freshly shaped line. Dropped when the observed generation is stale.
    pub(crate) fn recent_insert(
        &self,
        font_generation: usize,
        text: &SharedString,
        font_size: Pixels,
        runs: &[FontRun],
        layout: &LineLayout,
    ) {
        let size_bits = font_size.0.to_bits();
        let mut recent = self.recent.lock();
        ShapeCounters::bump(&self.counters.lock_acquisitions);
        if !recent.sync(font_generation) {
            return;
        }
        recent.insert(text, size_bits, runs, layout, &self.counters);
    }

    /// Shapes one line on the platform, counting it.
    pub(crate) fn platform_shape(
        &self,
        platform: &dyn PlatformTextSystem,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> LineLayout {
        ShapeCounters::bump(&self.counters.platform_line_shapes);
        platform.layout_line(text, font_size, runs)
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn stats(&self) -> ShapeStats {
        let counters = &self.counters;
        let recent = self.recent.lock();
        ShapeStats {
            platform_line_shapes: counters.platform_line_shapes.load(Ordering::Relaxed),
            recent_hits: counters.recent_hits.load(Ordering::Relaxed),
            recent_rotations: counters.recent_rotations.load(Ordering::Relaxed),
            lock_acquisitions: counters.lock_acquisitions.load(Ordering::Relaxed),
            retained_glyph_units: recent.current.units + recent.old.units,
        }
    }

    #[cfg(test)]
    pub(crate) fn force_recent_hash_collisions(&self) {
        self.recent.lock().constant_hash = Some(0);
    }

    #[cfg(test)]
    pub(crate) fn recent_unit_counts(&self) -> (usize, usize) {
        let recent = self.recent.lock();
        (recent.current.units, recent.old.units)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_system::{FontId, GlyphId, ShapedGlyph, ShapedRun};
    use crate::{point, px};

    fn layout_of(glyphs: usize) -> LineLayout {
        LineLayout {
            runs: if glyphs == 0 {
                Vec::new()
            } else {
                vec![ShapedRun {
                    font_id: FontId(0),
                    glyphs: (0..glyphs)
                        .map(|index| ShapedGlyph {
                            id: GlyphId(index as u32),
                            position: point(px(index as f32), px(0.)),
                            index,
                            is_emoji: false,
                        })
                        .collect(),
                }]
            },
            len: glyphs,
            width: px(glyphs as f32),
            ..Default::default()
        }
    }

    fn runs(len: usize) -> [FontRun; 1] {
        [FontRun {
            len,
            font_id: FontId(0),
        }]
    }

    fn insert(caches: &ShapeCaches, generation: usize, text: &str, glyphs: usize) {
        caches.recent_insert(
            generation,
            &SharedString::from(text.to_string()),
            px(14.),
            &runs(text.len()),
            &layout_of(glyphs),
        );
    }

    fn lookup(caches: &ShapeCaches, generation: usize, text: &str) -> Option<LineLayout> {
        caches.recent_lookup(generation, text, px(14.), &runs(text.len()))
    }

    #[test]
    fn rotation_happens_on_the_insertion_that_finds_the_generation_full() {
        let caches = ShapeCaches::default();
        insert(&caches, 0, "a", RECENT_GENERATION_GLYPH_UNITS - 1);
        // An empty line counts as one unit and fills the generation exactly.
        insert(&caches, 0, "b", 0);
        assert_eq!(caches.counters.recent_rotations.load(Ordering::Relaxed), 0);
        assert_eq!(
            caches.recent_unit_counts(),
            (RECENT_GENERATION_GLYPH_UNITS, 0)
        );

        insert(&caches, 0, "c", 3);
        assert_eq!(caches.counters.recent_rotations.load(Ordering::Relaxed), 1);
        assert_eq!(
            caches.recent_unit_counts(),
            (3, RECENT_GENERATION_GLYPH_UNITS)
        );
        assert!(lookup(&caches, 0, "a").is_some());
    }

    #[test]
    fn residency_stays_within_two_generations_plus_one_line() {
        let caches = ShapeCaches::default();
        let mut peak = 0;
        for ix in 0..400 {
            insert(&caches, 0, &format!("line {ix}"), 100);
            let (current, old) = caches.recent_unit_counts();
            peak = peak.max(current + old);
        }
        assert!(peak <= 2 * RECENT_GENERATION_GLYPH_UNITS + 100, "{peak}");
    }

    #[test]
    fn old_generation_hits_are_promoted_with_their_content() {
        let caches = ShapeCaches::default();
        insert(&caches, 0, "first", RECENT_GENERATION_GLYPH_UNITS);
        insert(&caches, 0, "second", 7);
        let (current, old) = caches.recent_unit_counts();
        assert_eq!((current, old), (7, RECENT_GENERATION_GLYPH_UNITS));

        let promoted = lookup(&caches, 0, "first").expect("old generation hit");
        assert_eq!(promoted.len, RECENT_GENERATION_GLYPH_UNITS);
        assert_eq!(promoted.runs[0].glyphs.len(), RECENT_GENERATION_GLYPH_UNITS);
        // The promotion went through the rotation test: the current generation was small, so it
        // simply received the entry.
        let (current, old) = caches.recent_unit_counts();
        assert_eq!(current, 7 + RECENT_GENERATION_GLYPH_UNITS);
        assert_eq!(old, 0);
        assert!(lookup(&caches, 0, "first").is_some());
        assert!(lookup(&caches, 0, "second").is_some());
    }

    #[test]
    fn promotion_into_a_full_generation_rotates_first() {
        let caches = ShapeCaches::default();
        insert(&caches, 0, "x", RECENT_GENERATION_GLYPH_UNITS);
        insert(&caches, 0, "y", 1);
        insert(&caches, 0, "z", RECENT_GENERATION_GLYPH_UNITS - 1);
        assert_eq!(
            caches.recent_unit_counts(),
            (RECENT_GENERATION_GLYPH_UNITS, RECENT_GENERATION_GLYPH_UNITS)
        );
        assert_eq!(caches.counters.recent_rotations.load(Ordering::Relaxed), 1);

        // "x" lives in the old generation; the current one is full, so promoting it rotates.
        assert_eq!(
            lookup(&caches, 0, "x").unwrap().len,
            RECENT_GENERATION_GLYPH_UNITS
        );
        assert_eq!(caches.counters.recent_rotations.load(Ordering::Relaxed), 2);
        assert_eq!(
            caches.recent_unit_counts(),
            (RECENT_GENERATION_GLYPH_UNITS, RECENT_GENERATION_GLYPH_UNITS)
        );
        assert!(lookup(&caches, 0, "y").is_some());
    }

    #[test]
    fn colliding_hashes_never_select_a_layout_by_hash_alone() {
        let caches = ShapeCaches::default();
        caches.force_recent_hash_collisions();
        insert(&caches, 0, "alpha", 5);
        insert(&caches, 0, "bravo", 6);
        insert(&caches, 0, "charlie", 7);
        assert_eq!(lookup(&caches, 0, "alpha").unwrap().len, 5);
        assert_eq!(lookup(&caches, 0, "bravo").unwrap().len, 6);
        assert_eq!(lookup(&caches, 0, "charlie").unwrap().len, 7);
        assert!(lookup(&caches, 0, "delta").is_none());
        // Same text and runs but another size is a different key.
        assert!(
            caches
                .recent_lookup(0, "alpha", px(15.), &runs(5))
                .is_none()
        );
    }

    #[test]
    fn a_newer_font_generation_clears_both_generations_and_stale_inserts_are_dropped() {
        let caches = ShapeCaches::default();
        insert(&caches, 0, "kept", 4);
        insert(&caches, 0, "second", RECENT_GENERATION_GLYPH_UNITS);
        insert(&caches, 0, "third", RECENT_GENERATION_GLYPH_UNITS);
        assert!(caches.recent_unit_counts().1 > 0);

        assert!(lookup(&caches, 1, "kept").is_none());
        assert_eq!(caches.recent_unit_counts(), (0, 0));

        // An observer of the stale generation neither reads nor writes.
        insert(&caches, 0, "stale", 3);
        assert_eq!(caches.recent_unit_counts(), (0, 0));
        assert!(lookup(&caches, 0, "stale").is_none());

        insert(&caches, 1, "fresh", 3);
        assert!(lookup(&caches, 1, "fresh").is_some());
    }
}
