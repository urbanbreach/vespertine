//! Bounded recent shaping store (PRF-012), numeric shaping from character pairs (PRF-013) and
//! shaping of appended text from the layout of the line it extends.
//!
//! Both pieces of state belong to one [`TextSystem`](crate::TextSystem) and are shared by every
//! window text system built on it, so they are guarded by locks. A lock is never held while the
//! platform shapes text.

use super::{FontId, FontRun, GlyphId, LineLayout, ShapedGlyph, ShapedRun};
use crate::{Pixels, PlatformTextSystem, Point, SharedString, point, px};
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

/// Font sizes whose learned characters and pairs are kept (about 1 MB). Beyond this the least
/// recently used size's table is dropped; that size is learned again if it returns.
pub(crate) const NUMERIC_MAX_FONT_SIZES: usize = 256;

const NUMERIC_ALPHABET: &[u8; 20] = b"0123456789.,+-%$KMBT";

/// Shortest line, in bytes, whose growth is shaped incrementally.
pub(crate) const APPEND_MIN_LEN: usize = 48;

/// Longest line, in bytes, kept for incremental shaping.
pub(crate) const APPEND_MAX_LEN: usize = 16 * 1024;

/// Growing lines tracked at once.
pub(crate) const APPEND_TAILS: usize = 4;

/// Longest chain of lines that each extend the previous one's layout before the next line is
/// shaped in full again. Every link adds the rounding error of one addition to the positions
/// of the words it shapes; the full shaping anchors the chain.
pub(crate) const APPEND_MAX_CHAIN: u32 = 32;

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
    pub(crate) appended_lines: AtomicU64,
    /// Lines that missed the frame cache and the recent-shape store and were shaped (platform
    /// or numeric assembly), whichever way. Feeds `FrameStats::lines_shaped` (DEV-003).
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
    /// Glyph units currently retained by both recent generations.
    pub retained_glyph_units: usize,
    /// Lines assembled from the layout of a shorter line they extend and a platform-shaped tail.
    pub appended_lines: u64,
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
    /// The advance of the first character as the platform adjusts it for the second one: the
    /// position of the second glyph when the pair is shaped alone.
    Known(Pixels),
}

struct NumericFont {
    chars: [CharState; 20],
    pairs: Box<[[PairState; 20]; 20]>,
    /// The [`NumericState::uses`] count at the last lookup of this table.
    used: u64,
}

impl NumericFont {
    fn new() -> Self {
        Self {
            chars: [CharState::Unknown; 20],
            pairs: Box::new([[PairState::Unknown; 20]; 20]),
            used: 0,
        }
    }
}

#[derive(Default)]
struct NumericState {
    font_generation: usize,
    fonts: FxHashMap<(FontId, u32), NumericFont>,
    /// Table lookups so far; orders the tables by recency.
    uses: u64,
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

    /// The table of `key`, marked as just used. A missing table is created, after dropping the
    /// least recently used one when [`NUMERIC_MAX_FONT_SIZES`] tables exist.
    fn table(&mut self, key: (FontId, u32)) -> &mut NumericFont {
        self.uses += 1;
        let uses = self.uses;
        if self.fonts.len() >= NUMERIC_MAX_FONT_SIZES
            && !self.fonts.contains_key(&key)
            && let Some(oldest) = self
                .fonts
                .iter()
                .min_by_key(|(_, font)| font.used)
                .map(|(key, _)| *key)
        {
            self.fonts.remove(&oldest);
        }
        let font = self.fonts.entry(key).or_insert_with(NumericFont::new);
        font.used = uses;
        font
    }
}

fn alphabet_index(byte: u8) -> Option<usize> {
    NUMERIC_ALPHABET.iter().position(|&b| b == byte)
}

/// A recently shaped line that a longer line may continue.
struct Tail {
    text: SharedString,
    size_bits: u32,
    font_id: FontId,
    layout: LineLayout,
    /// Lines assembled since the layout was shaped in full.
    chain: u32,
}

/// The growing lines tracked for incremental shaping.
#[derive(Default)]
struct AppendState {
    font_generation: usize,
    tails: SmallVec<[Tail; APPEND_TAILS]>,
    /// The tail replaced next when a new line starts while every slot is taken.
    victim: usize,
}

impl AppendState {
    fn sync(&mut self, font_generation: usize) -> bool {
        if font_generation > self.font_generation {
            self.tails.clear();
            self.font_generation = font_generation;
            true
        } else {
            font_generation == self.font_generation
        }
    }

    fn remember(&mut self, tail: Tail) {
        if self.tails.len() < APPEND_TAILS {
            self.tails.push(tail);
        } else {
            self.victim %= APPEND_TAILS;
            self.tails[self.victim] = tail;
            self.victim += 1;
        }
    }
}

/// Whether `bytes` holds only printable ASCII: a single left-to-right script run whose words
/// the platform shapes independently of their neighbours.
fn is_printable_ascii(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| (0x20..0x7F).contains(byte))
}

/// The one font of a line that is a single run of `text`.
fn single_run_font(text: &str, runs: &[FontRun]) -> Option<FontId> {
    match runs {
        [run] if run.len == text.len() => Some(run.font_id),
        _ => None,
    }
}

/// The largest split position `p <= limit` at which `text` can be cut into two independently
/// shaped parts: a single space between two ASCII alphanumerics, with `text[p]` the first
/// character of the second part. Such a position is a line-break opportunity whatever precedes
/// it, so the platform shapes the words on both sides separately.
fn split_position(text: &[u8], limit: usize) -> Option<usize> {
    let limit = limit.min(text.len().checked_sub(1)?);
    (2..=limit).rev().find(|&p| {
        text[p - 1] == b' '
            && text[p - 2].is_ascii_alphanumeric()
            && text[p].is_ascii_alphanumeric()
    })
}

/// A tail shaped in place of a full line; see [`ShapeCaches::append_layout`].
struct Continuation {
    tail: usize,
    chain: u32,
    split: usize,
    /// Where the tail places the glyph that `text[split..]` starts with.
    anchor: Point<Pixels>,
    glyphs: Vec<ShapedGlyph>,
    ascent: Pixels,
    descent: Pixels,
    font_id: FontId,
}

/// Shaping state shared by every window text system of one text system.
pub(crate) struct ShapeCaches {
    recent: Mutex<RecentShapes>,
    numeric: Mutex<NumericState>,
    append: Mutex<AppendState>,
    pub(crate) counters: ShapeCounters,
    numeric_enabled: std::sync::atomic::AtomicBool,
    timing_enabled: std::sync::atomic::AtomicBool,
}

impl Default for ShapeCaches {
    fn default() -> Self {
        Self {
            recent: Mutex::default(),
            numeric: Mutex::default(),
            append: Mutex::default(),
            counters: ShapeCounters::default(),
            numeric_enabled: std::sync::atomic::AtomicBool::new(true),
            timing_enabled: std::sync::atomic::AtomicBool::new(false),
        }
    }
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

    /// Remembers a line shaped in full so that a longer line extending it can be shaped
    /// incrementally. It replaces the tracked line it extends, if any.
    pub(crate) fn append_track(
        &self,
        platform: &dyn PlatformTextSystem,
        font_generation: usize,
        text: &SharedString,
        font_size: Pixels,
        runs: &[FontRun],
        layout: &LineLayout,
    ) {
        if !platform.shapes_ascii_words_independently() {
            return;
        }
        let Some(tail) = Self::tail_of(text, font_size, runs, layout, 0) else {
            return;
        };
        let mut append = self.append.lock();
        ShapeCounters::bump(&self.counters.lock_acquisitions);
        if !append.sync(font_generation) {
            return;
        }
        let bytes = text.as_bytes();
        match append.tails.iter_mut().find(|known| {
            known.size_bits == tail.size_bits
                && known.font_id == tail.font_id
                && known.text.len() <= text.len()
                && bytes.starts_with(known.text.as_bytes())
        }) {
            Some(known) => *known = tail,
            None => append.remember(tail),
        }
    }

    /// The tail record of a line eligible for incremental shaping.
    fn tail_of(
        text: &SharedString,
        font_size: Pixels,
        runs: &[FontRun],
        layout: &LineLayout,
        chain: u32,
    ) -> Option<Tail> {
        let font_id = single_run_font(text, runs)?;
        let eligible = (APPEND_MIN_LEN..=APPEND_MAX_LEN).contains(&text.len())
            && is_printable_ascii(text.as_bytes())
            && layout.len == text.len()
            && matches!(layout.runs.as_slice(), [run]
                if run.font_id == font_id
                    && !run.glyphs.is_empty()
                    && run.glyphs.iter().all(|glyph| glyph.id != GlyphId(0)));
        eligible.then(|| Tail {
            text: text.clone(),
            size_bits: font_size.0.to_bits(),
            font_id,
            layout: layout.clone(),
            chain,
        })
    }

    /// Shapes `text` as the layout of a shorter, recently shaped line it extends plus the
    /// platform shaping of only the words after the last shared word boundary.
    ///
    /// Only printable ASCII in a single font run qualifies, and only on a platform that
    /// promises to shape such words independently
    /// ([`PlatformTextSystem::shapes_ascii_words_independently`]): the shared prefix then keeps
    /// its glyphs and positions, and the shaped remainder moves by the advance of that prefix.
    /// The result is the platform's own layout of `text`, up to the rounding of adding the
    /// prefix advance; [`APPEND_MAX_CHAIN`] bounds how often that rounding repeats.
    pub(crate) fn append_layout(
        &self,
        platform: &dyn PlatformTextSystem,
        font_generation: usize,
        text: &SharedString,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> Option<LineLayout> {
        let font_id = single_run_font(text, runs)?;
        if !(APPEND_MIN_LEN..=APPEND_MAX_LEN).contains(&text.len())
            || !platform.shapes_ascii_words_independently()
        {
            return None;
        }
        let size_bits = font_size.0.to_bits();
        let bytes = text.as_bytes();

        let found = {
            let mut append = self.append.lock();
            ShapeCounters::bump(&self.counters.lock_acquisitions);
            if !append.sync(font_generation) {
                return None;
            }
            append.tails.iter().enumerate().find_map(|(index, tail)| {
                if tail.size_bits != size_bits
                    || tail.font_id != font_id
                    || tail.chain >= APPEND_MAX_CHAIN
                    || tail.text.len() >= text.len()
                    || !bytes.starts_with(tail.text.as_bytes())
                {
                    return None;
                }
                // A split inside the tail, so the tail holds the glyph the split starts with.
                let split = split_position(bytes, tail.text.len() - 1)?;
                let [run] = tail.layout.runs.as_slice() else {
                    return None;
                };
                let kept = run.glyphs.partition_point(|glyph| glyph.index < split);
                let anchor = match run.glyphs.get(kept) {
                    Some(glyph) if glyph.index == split => glyph.position,
                    _ => return None,
                };
                Some(Continuation {
                    tail: index,
                    chain: tail.chain + 1,
                    split,
                    anchor,
                    glyphs: run.glyphs[..kept].to_vec(),
                    ascent: tail.layout.ascent,
                    descent: tail.layout.descent,
                    font_id,
                })
            })
        }?;
        if !is_printable_ascii(&bytes[found.split..]) {
            return None;
        }

        let rest = &text[found.split..];
        let rest_layout = self.platform_shape(
            platform,
            rest,
            font_size,
            &[FontRun {
                len: rest.len(),
                font_id,
            }],
        );
        let [rest_run] = rest_layout.runs.as_slice() else {
            return None;
        };
        if rest_run.font_id != found.font_id
            || rest_layout.len != rest.len()
            || rest_layout.ascent != found.ascent
            || rest_layout.descent != found.descent
            || rest_run.glyphs.is_empty()
            || rest_run.glyphs.iter().any(|glyph| glyph.id == GlyphId(0))
        {
            return None;
        }

        // The tail places the first glyph of `text[split..]` at the advance of `text[..split]`
        // plus that glyph's own position in the separately shaped remainder.
        let offset = found.anchor - rest_run.glyphs[0].position;
        let mut glyphs = found.glyphs;
        glyphs.extend(rest_run.glyphs.iter().map(|glyph| ShapedGlyph {
            id: glyph.id,
            position: glyph.position + offset,
            index: glyph.index + found.split,
            is_emoji: glyph.is_emoji,
        }));
        let layout = LineLayout {
            font_size,
            width: offset.x + rest_layout.width,
            ascent: found.ascent,
            descent: found.descent,
            runs: vec![ShapedRun {
                font_id: found.font_id,
                glyphs,
            }],
            len: text.len(),
        };
        ShapeCounters::bump(&self.counters.appended_lines);

        // The extended line replaces the line it extends as the tail of the lineage.
        if let Some(tail) = Self::tail_of(text, font_size, runs, &layout, found.chain) {
            let mut append = self.append.lock();
            ShapeCounters::bump(&self.counters.lock_acquisitions);
            if append.sync(font_generation) {
                match append.tails.get_mut(found.tail) {
                    Some(slot) if bytes.starts_with(slot.text.as_bytes()) => *slot = tail,
                    _ => append.remember(tail),
                }
            }
        }
        Some(layout)
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

    /// Assembles `text` from per-character glyphs and pair advances when it is eligible and the
    /// platform proves that it shapes the line pair by pair
    /// ([`PlatformTextSystem::shapes_pairwise`]). Every glyph position is then the platform's
    /// own sum of the same advances, so the line equals full platform shaping.
    pub(crate) fn numeric_layout(
        &self,
        platform: &dyn PlatformTextSystem,
        font_generation: usize,
        text: &str,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> Option<LineLayout> {
        if !self.numeric_enabled.load(Ordering::Relaxed)
            || text.is_empty()
            || text.len() > NUMERIC_MAX_LEN
            || runs.len() != 1
            || runs[0].len != text.len()
        {
            return None;
        }
        let bytes = text.as_bytes();
        let mut indices = [0u8; NUMERIC_MAX_LEN];
        for (slot, &byte) in indices.iter_mut().zip(bytes) {
            *slot = alphabet_index(byte)? as u8;
        }
        let indices = &indices[..bytes.len()];
        let font_id = runs[0].font_id;
        if !platform.shapes_pairwise(text, font_id) {
            return None;
        }
        let key = (font_id, font_size.0.to_bits());

        // Each pass either assembles the line or learns the characters and pairs it lacks.
        for _ in 0..4 {
            let mut missing_chars = SmallVec::<[u8; 20]>::new();
            let mut missing_pairs = SmallVec::<[(u8, u8); 16]>::new();
            {
                let mut numeric = self.numeric.lock();
                ShapeCounters::bump(&self.counters.lock_acquisitions);
                if !numeric.sync(font_generation) {
                    return None;
                }
                let font = numeric.table(key);

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
                    return None;
                }
                if missing_chars.is_empty() && missing_pairs.is_empty() {
                    ShapeCounters::bump(&self.counters.synthesized_lines);
                    return Some(assemble(font, indices, font_id, font_size));
                }
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
                    numeric.fonts.get(&key)?.chars
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
                            if platform.shapes_pairwise(text, font_id) {
                                let shaped = self.unit_shape(platform, text, font_size, font_id);
                                measure_pair(&shaped, font_id, &first_unit, &second_unit)
                            } else {
                                PairState::Unsupported
                            }
                        }
                        _ => PairState::Unsupported,
                    };
                    pair_results.push((first, second, state));
                }
            }

            let mut numeric = self.numeric.lock();
            ShapeCounters::bump(&self.counters.lock_acquisitions);
            if !numeric.sync(font_generation) {
                return None;
            }
            let font = numeric.table(key);
            for (ix, state) in char_results {
                font.chars[ix as usize] = state;
            }
            for (first, second, state) in pair_results {
                font.pairs[first as usize][second as usize] = state;
            }
        }
        None
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
            retained_glyph_units: recent.current.units + recent.old.units,
            appended_lines: counters.appended_lines.load(Ordering::Relaxed),
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
        // The platform places the second glyph by its isolated advance: nothing reaches across.
        || shaped.width != second_glyph.position.x + second.advance
    {
        return PairState::Unsupported;
    }
    PairState::Known(second_glyph.position.x)
}

/// The line of `indices` as the platform places it: glyph `i` at the sum of the pair advances
/// before it, the width that sum plus the last character's advance.
fn assemble(font: &NumericFont, indices: &[u8], font_id: FontId, font_size: Pixels) -> LineLayout {
    let unit = |ix: u8| match font.chars[ix as usize] {
        CharState::Known(unit) => unit,
        _ => unreachable!("assembled only when every character is known"),
    };
    let mut glyphs = Vec::with_capacity(indices.len());
    let mut x = px(0.);
    for (position, &ix) in indices.iter().enumerate() {
        if position > 0 {
            let previous = indices[position - 1];
            let PairState::Known(advance) = font.pairs[previous as usize][ix as usize] else {
                unreachable!("assembled only when every pair is known");
            };
            x += advance;
        }
        let unit = unit(ix);
        glyphs.push(ShapedGlyph {
            id: unit.glyph,
            position: point(x, px(0.)),
            index: position,
            is_emoji: unit.is_emoji,
        });
    }
    let first = unit(indices[0]);
    let last = unit(indices[indices.len() - 1]);
    LineLayout {
        font_size,
        width: x + last.advance,
        ascent: first.ascent,
        descent: first.descent,
        runs: vec![ShapedRun { font_id, glyphs }],
        len: indices.len(),
    }
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
