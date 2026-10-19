//! Bounded recent shaping store (PRF-012) and validated numeric shaping (PRF-013).
//!
//! Both pieces of state belong to one [`TextSystem`](crate::TextSystem) and are shared by every
//! window text system built on it, so they are guarded by locks. A lock is never held while the
//! platform shapes text.

use super::{FontId, FontRun, GlyphId, LineLayout, ShapedGlyph, ShapedRun};
use crate::{Pixels, PlatformTextSystem, SharedString, point, px};
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::{
    hash::{Hash, Hasher},
    sync::atomic::{AtomicU64, Ordering},
};
use vespertine_collections::{FxHashMap, FxHasher};

/// Glyph units one generation of recent shapes may hold before the generations rotate.
pub(crate) const RECENT_GENERATION_GLYPH_UNITS: usize = 16_384;

/// Longest line, in bytes, that numeric shaping assembles.
pub(crate) const NUMERIC_MAX_LEN: usize = 64;

/// Lines compared with full platform shaping before sampling starts.
pub(crate) const NUMERIC_INITIAL_CHECKS: u64 = 32;

/// After the initial checks, every n-th synthesized line is compared.
pub(crate) const NUMERIC_CHECK_INTERVAL: u64 = 256;

/// Agreement tolerance between synthesized and platform lines, in logical pixels.
pub(crate) const NUMERIC_TOLERANCE: f32 = 0.001;

const NUMERIC_ALPHABET: &[u8; 20] = b"0123456789.,+-%$KMBT";

/// Counters describing shaping work. Compiled always (relaxed atomics), read only by tests and
/// diagnostics.
#[derive(Default)]
pub(crate) struct ShapeCounters {
    pub(crate) platform_line_shapes: AtomicU64,
    pub(crate) numeric_unit_shapes: AtomicU64,
    pub(crate) recent_hits: AtomicU64,
    pub(crate) recent_rotations: AtomicU64,
    pub(crate) lock_acquisitions: AtomicU64,
    pub(crate) synthesized_lines: AtomicU64,
    pub(crate) validation_checks: AtomicU64,
    pub(crate) disabled_font_sizes: AtomicU64,
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
    /// Full lines handed to the platform text system (including validation shapes).
    pub platform_line_shapes: u64,
    /// One- and two-character measurements made for numeric shaping.
    pub numeric_unit_shapes: u64,
    /// Lines served by the recent-shape store.
    pub recent_hits: u64,
    /// Generation rotations of the recent-shape store.
    pub recent_rotations: u64,
    /// Acquisitions of the recent-shape and numeric-state locks.
    pub lock_acquisitions: u64,
    /// Lines assembled from characters and pairs.
    pub synthesized_lines: u64,
    /// Synthesized lines compared with platform shaping.
    pub validation_checks: u64,
    /// Font and size pairs whose numeric shaping was permanently disabled.
    pub disabled_font_sizes: u64,
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

#[derive(Clone, Copy)]
struct CharUnit {
    glyph: GlyphId,
    advance: Pixels,
    ascent: Pixels,
    descent: Pixels,
    is_emoji: bool,
}

#[derive(Clone, Copy)]
enum CharState {
    Unknown,
    Unsupported,
    Known(CharUnit),
}

#[derive(Clone, Copy)]
enum PairState {
    Unknown,
    Unsupported,
    Known(Pixels),
}

struct NumericFont {
    chars: [CharState; 20],
    pairs: Box<[[PairState; 20]; 20]>,
    synthesized: u64,
    disabled: bool,
}

impl NumericFont {
    fn new() -> Self {
        Self {
            chars: [CharState::Unknown; 20],
            pairs: Box::new([[PairState::Unknown; 20]; 20]),
            synthesized: 0,
            disabled: false,
        }
    }
}

#[derive(Default)]
struct NumericState {
    font_generation: usize,
    fonts: FxHashMap<(FontId, u32), NumericFont>,
}

impl NumericState {
    fn sync(&mut self, font_generation: usize) -> bool {
        if font_generation > self.font_generation {
            self.fonts.clear();
            self.font_generation = font_generation;
            true
        } else {
            font_generation == self.font_generation
        }
    }
}

fn alphabet_index(byte: u8) -> Option<usize> {
    NUMERIC_ALPHABET.iter().position(|&b| b == byte)
}

/// The outcome of asking numeric shaping for a line.
pub(crate) enum NumericOutcome {
    /// The line is not eligible, or a unit is unsupported: shape it on the platform.
    Platform,
    /// The line was assembled or validated; the layout is ready.
    Layout(LineLayout),
}

/// Shaping state shared by every window text system of one text system.
pub(crate) struct ShapeCaches {
    recent: Mutex<RecentShapes>,
    numeric: Mutex<NumericState>,
    pub(crate) counters: ShapeCounters,
    numeric_enabled: std::sync::atomic::AtomicBool,
}

impl Default for ShapeCaches {
    fn default() -> Self {
        Self {
            recent: Mutex::default(),
            numeric: Mutex::default(),
            counters: ShapeCounters::default(),
            numeric_enabled: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

impl ShapeCaches {
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

    /// Enables or disables numeric synthesis (diagnostic configuration of the workload).
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn set_numeric_enabled(&self, enabled: bool) {
        self.numeric_enabled.store(enabled, Ordering::Relaxed);
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

    fn unit_shape(
        &self,
        platform: &dyn PlatformTextSystem,
        text: &str,
        font_size: Pixels,
        font_id: FontId,
    ) -> LineLayout {
        ShapeCounters::bump(&self.counters.numeric_unit_shapes);
        platform.layout_line(
            text,
            font_size,
            &[FontRun {
                len: text.len(),
                font_id,
            }],
        )
    }

    /// Assembles `text` from per-character glyphs and pair kerning when it is eligible,
    /// validating against full platform shaping as PRF-013 requires.
    pub(crate) fn numeric_layout(
        &self,
        platform: &dyn PlatformTextSystem,
        font_generation: usize,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> NumericOutcome {
        if !self.numeric_enabled.load(Ordering::Relaxed)
            || text.is_empty()
            || text.len() > NUMERIC_MAX_LEN
            || runs.len() != 1
            || runs[0].len != text.len()
        {
            return NumericOutcome::Platform;
        }
        let bytes = text.as_bytes();
        let mut indices = [0u8; NUMERIC_MAX_LEN];
        for (slot, &byte) in indices.iter_mut().zip(bytes) {
            match alphabet_index(byte) {
                Some(ix) => *slot = ix as u8,
                None => return NumericOutcome::Platform,
            }
        }
        let indices = &indices[..bytes.len()];
        let font_id = runs[0].font_id;
        let key = (font_id, font_size.0.to_bits());

        // Each pass either assembles the line or learns the characters and pairs it lacks.
        for _ in 0..4 {
            let mut missing_chars = SmallVec::<[u8; 20]>::new();
            let mut missing_pairs = SmallVec::<[(u8, u8); 16]>::new();
            let (layout, validate) = {
                let mut numeric = self.numeric.lock();
                ShapeCounters::bump(&self.counters.lock_acquisitions);
                if !numeric.sync(font_generation) {
                    return NumericOutcome::Platform;
                }
                let font = numeric.fonts.entry(key).or_insert_with(NumericFont::new);
                if font.disabled {
                    return NumericOutcome::Platform;
                }

                let mut unsupported = false;
                for (position, &ix) in indices.iter().enumerate() {
                    match font.chars[ix as usize] {
                        CharState::Unknown => {
                            if !missing_chars.contains(&ix) {
                                missing_chars.push(ix);
                            }
                        }
                        CharState::Unsupported => unsupported = true,
                        CharState::Known(_) => {}
                    }
                    if position > 0 {
                        let previous = indices[position - 1];
                        match font.pairs[previous as usize][ix as usize] {
                            PairState::Unknown => {
                                if !missing_pairs.contains(&(previous, ix)) {
                                    missing_pairs.push((previous, ix));
                                }
                            }
                            PairState::Unsupported => unsupported = true,
                            PairState::Known(_) => {}
                        }
                    }
                }
                if unsupported {
                    return NumericOutcome::Platform;
                }
                if missing_chars.is_empty() && missing_pairs.is_empty() {
                    let layout = assemble(font, indices, font_id, font_size);
                    font.synthesized += 1;
                    let count = font.synthesized;
                    let validate =
                        count <= NUMERIC_INITIAL_CHECKS || count % NUMERIC_CHECK_INTERVAL == 0;
                    (layout, validate)
                } else {
                    (LineLayout::default(), false)
                }
            };

            if missing_chars.is_empty() && missing_pairs.is_empty() {
                ShapeCounters::bump(&self.counters.synthesized_lines);
                if !validate {
                    return NumericOutcome::Layout(layout);
                }
                ShapeCounters::bump(&self.counters.validation_checks);
                let platform_layout = self.platform_shape(platform, text, font_size, runs);
                if layouts_agree(&layout, &platform_layout) {
                    return NumericOutcome::Layout(layout);
                }
                let mut numeric = self.numeric.lock();
                ShapeCounters::bump(&self.counters.lock_acquisitions);
                if numeric.sync(font_generation)
                    && let Some(font) = numeric.fonts.get_mut(&key)
                    && !font.disabled
                {
                    font.disabled = true;
                    ShapeCounters::bump(&self.counters.disabled_font_sizes);
                }
                return NumericOutcome::Layout(platform_layout);
            }

            // Measure outside the lock.
            let mut char_results = SmallVec::<[(u8, CharState); 20]>::new();
            for &ix in &missing_chars {
                let character = NUMERIC_ALPHABET[ix as usize] as char;
                let mut buffer = [0u8; 4];
                let isolated = self.unit_shape(
                    platform,
                    character.encode_utf8(&mut buffer),
                    font_size,
                    font_id,
                );
                char_results.push((ix, measure_char(&isolated, font_id)));
            }
            let mut pair_results = SmallVec::<[(u8, u8, PairState); 16]>::new();
            if !missing_pairs.is_empty() {
                // Pair measurements need the isolated units of both characters.
                let units = {
                    let numeric = self.numeric.lock();
                    ShapeCounters::bump(&self.counters.lock_acquisitions);
                    let Some(font) = numeric.fonts.get(&key) else {
                        return NumericOutcome::Platform;
                    };
                    font.chars
                };
                let unit_of = |ix: u8| -> CharState {
                    char_results
                        .iter()
                        .find(|(char_ix, _)| *char_ix == ix)
                        .map(|(_, state)| *state)
                        .unwrap_or(units[ix as usize])
                };
                for &(first, second) in &missing_pairs {
                    let state = match (unit_of(first), unit_of(second)) {
                        (CharState::Known(first_unit), CharState::Known(second_unit)) => {
                            let pair = [
                                NUMERIC_ALPHABET[first as usize],
                                NUMERIC_ALPHABET[second as usize],
                            ];
                            let text = std::str::from_utf8(&pair).unwrap_or("");
                            let shaped = self.unit_shape(platform, text, font_size, font_id);
                            measure_pair(&shaped, font_id, &first_unit, &second_unit)
                        }
                        _ => PairState::Unsupported,
                    };
                    pair_results.push((first, second, state));
                }
            }

            let mut numeric = self.numeric.lock();
            ShapeCounters::bump(&self.counters.lock_acquisitions);
            if !numeric.sync(font_generation) {
                return NumericOutcome::Platform;
            }
            let font = numeric.fonts.entry(key).or_insert_with(NumericFont::new);
            for (ix, state) in char_results {
                font.chars[ix as usize] = state;
            }
            for (first, second, state) in pair_results {
                font.pairs[first as usize][second as usize] = state;
            }
        }
        NumericOutcome::Platform
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn stats(&self) -> ShapeStats {
        let counters = &self.counters;
        let recent = self.recent.lock();
        ShapeStats {
            platform_line_shapes: counters.platform_line_shapes.load(Ordering::Relaxed),
            numeric_unit_shapes: counters.numeric_unit_shapes.load(Ordering::Relaxed),
            recent_hits: counters.recent_hits.load(Ordering::Relaxed),
            recent_rotations: counters.recent_rotations.load(Ordering::Relaxed),
            lock_acquisitions: counters.lock_acquisitions.load(Ordering::Relaxed),
            synthesized_lines: counters.synthesized_lines.load(Ordering::Relaxed),
            validation_checks: counters.validation_checks.load(Ordering::Relaxed),
            disabled_font_sizes: counters.disabled_font_sizes.load(Ordering::Relaxed),
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

    #[cfg(test)]
    pub(crate) fn numeric_is_disabled(&self, font_id: FontId, font_size: Pixels) -> bool {
        self.numeric
            .lock()
            .fonts
            .get(&(font_id, font_size.0.to_bits()))
            .is_some_and(|font| font.disabled)
    }

    #[cfg(test)]
    pub(crate) fn numeric_has_state(&self) -> bool {
        !self.numeric.lock().fonts.is_empty()
    }
}

fn measure_char(isolated: &LineLayout, font_id: FontId) -> CharState {
    let [run] = isolated.runs.as_slice() else {
        return CharState::Unsupported;
    };
    let [glyph] = run.glyphs.as_slice() else {
        return CharState::Unsupported;
    };
    if run.font_id != font_id
        || glyph.position != point(px(0.), px(0.))
        || glyph.index != 0
        || isolated.len != 1
    {
        return CharState::Unsupported;
    }
    CharState::Known(CharUnit {
        glyph: glyph.id,
        advance: isolated.width,
        ascent: isolated.ascent,
        descent: isolated.descent,
        is_emoji: glyph.is_emoji,
    })
}

fn measure_pair(
    shaped: &LineLayout,
    font_id: FontId,
    first: &CharUnit,
    second: &CharUnit,
) -> PairState {
    let [run] = shaped.runs.as_slice() else {
        return PairState::Unsupported;
    };
    let [first_glyph, second_glyph] = run.glyphs.as_slice() else {
        return PairState::Unsupported;
    };
    if run.font_id != font_id
        || first_glyph.id != first.glyph
        || second_glyph.id != second.glyph
        || first_glyph.index != 0
        || second_glyph.index != 1
        || first_glyph.position != point(px(0.), px(0.))
        || second_glyph.position.y != px(0.)
    {
        return PairState::Unsupported;
    }
    let expected_width = second_glyph.position.x + second.advance;
    if (shaped.width - expected_width).0.abs() > NUMERIC_TOLERANCE {
        return PairState::Unsupported;
    }
    PairState::Known(second_glyph.position.x - first.advance)
}

fn assemble(font: &NumericFont, indices: &[u8], font_id: FontId, font_size: Pixels) -> LineLayout {
    let mut glyphs = Vec::with_capacity(indices.len());
    let mut x = px(0.);
    let mut previous: Option<(u8, CharUnit)> = None;
    let mut first_unit = None;
    for (position, &ix) in indices.iter().enumerate() {
        let CharState::Known(unit) = font.chars[ix as usize] else {
            unreachable!("assembled only when every character is known");
        };
        if let Some((previous_ix, previous_unit)) = previous {
            let PairState::Known(kerning) = font.pairs[previous_ix as usize][ix as usize] else {
                unreachable!("assembled only when every pair is known");
            };
            x = x + previous_unit.advance + kerning;
        }
        glyphs.push(ShapedGlyph {
            id: unit.glyph,
            position: point(x, px(0.)),
            index: position,
            is_emoji: unit.is_emoji,
        });
        first_unit.get_or_insert(unit);
        previous = Some((ix, unit));
    }
    let last = previous
        .map(|(_, unit)| unit)
        .unwrap_or_else(|| unreachable!());
    let first = first_unit.unwrap_or(last);
    LineLayout {
        font_size,
        width: x + last.advance,
        ascent: first.ascent,
        descent: first.descent,
        runs: vec![ShapedRun { font_id, glyphs }],
        len: indices.len(),
    }
}

/// Whether two layouts agree under the PRF-013 validation rule.
pub(crate) fn layouts_agree(synthesized: &LineLayout, platform: &LineLayout) -> bool {
    let close = |a: Pixels, b: Pixels| (a - b).0.abs() <= NUMERIC_TOLERANCE;
    if synthesized.len != platform.len
        || synthesized.font_size != platform.font_size
        || !close(synthesized.width, platform.width)
        || synthesized.ascent != platform.ascent
        || synthesized.descent != platform.descent
        || synthesized.runs.len() != platform.runs.len()
    {
        return false;
    }
    synthesized.runs.iter().zip(&platform.runs).all(|(a, b)| {
        a.font_id == b.font_id
            && a.glyphs.len() == b.glyphs.len()
            && a.glyphs.iter().zip(&b.glyphs).all(|(x, y)| {
                x.id == y.id
                    && x.index == y.index
                    && x.is_emoji == y.is_emoji
                    && close(x.position.x, y.position.x)
                    && close(x.position.y, y.position.y)
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
