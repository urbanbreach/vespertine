use crate::{FontId, GlyphId, Pixels, PlatformTextSystem, Point, SharedString, Size, point, px};
use parking_lot::{Mutex, RwLock, RwLockUpgradableReadGuard};
use smallvec::SmallVec;
use std::{
    borrow::Borrow,
    hash::{Hash, Hasher},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use vespertine_collections::FxHashMap;

use super::{
    LineWrapper,
    shape_cache::{NumericOutcome, ShapeCaches},
};

/// A laid out and styled line of text
#[derive(Default, Debug, Clone)]
pub struct LineLayout {
    /// The font size for this line
    pub font_size: Pixels,
    /// The width of the line
    pub width: Pixels,
    /// The ascent of the line
    pub ascent: Pixels,
    /// The descent of the line
    pub descent: Pixels,
    /// The shaped runs that make up this line
    pub runs: Vec<ShapedRun>,
    /// The length of the line in utf-8 bytes
    pub len: usize,
}

/// A run of text that has been shaped .
#[derive(Debug, Clone)]
pub struct ShapedRun {
    /// The font id for this run
    pub font_id: FontId,
    /// The glyphs that make up this run
    pub glyphs: Vec<ShapedGlyph>,
}

/// A single glyph, ready to paint.
#[derive(Clone, Debug)]
pub struct ShapedGlyph {
    /// The ID for this glyph, as determined by the text system.
    pub id: GlyphId,

    /// The position of this glyph in its containing line.
    pub position: Point<Pixels>,

    /// The index of this glyph in the original text.
    pub index: usize,

    /// Whether this glyph is an emoji
    pub is_emoji: bool,
}

impl LineLayout {
    /// The index for the character at the given x coordinate
    pub fn index_for_x(&self, x: Pixels) -> Option<usize> {
        if x >= self.width {
            None
        } else {
            for run in self.runs.iter().rev() {
                for glyph in run.glyphs.iter().rev() {
                    if glyph.position.x <= x {
                        return Some(glyph.index);
                    }
                }
            }
            Some(0)
        }
    }

    /// closest_index_for_x returns the character boundary closest to the given x coordinate
    /// (e.g. to handle aligning up/down arrow keys)
    pub fn closest_index_for_x(&self, x: Pixels) -> usize {
        let mut prev_index = 0;
        let mut prev_x = px(0.);

        for run in self.runs.iter() {
            for glyph in run.glyphs.iter() {
                if glyph.position.x >= x {
                    if glyph.position.x - x < x - prev_x {
                        return glyph.index;
                    } else {
                        return prev_index;
                    }
                }
                prev_index = glyph.index;
                prev_x = glyph.position.x;
            }
        }

        if self.len == 1 {
            if x > self.width / 2. {
                return 1;
            } else {
                return 0;
            }
        }

        self.len
    }

    /// The x position of the character at the given index
    pub fn x_for_index(&self, index: usize) -> Pixels {
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.index >= index {
                    return glyph.position.x;
                }
            }
        }
        self.width
    }

    /// The corresponding Font at the given index
    pub fn font_id_for_index(&self, index: usize) -> Option<FontId> {
        for run in &self.runs {
            for glyph in &run.glyphs {
                if glyph.index >= index {
                    return Some(run.font_id);
                }
            }
        }
        None
    }

    /// Split this layout at a byte index, returning `(prefix, suffix)`.
    ///
    /// - `prefix` contains glyphs for bytes `[0, byte_index)` with original positions.
    ///   Its width equals the x-advance up to the split point.
    /// - `suffix` contains glyphs for bytes `[byte_index, len)` with positions
    ///   shifted left so the first glyph starts at x=0, and byte indices rebased to 0.
    /// - `font_size`, `ascent`, and `descent` are copied to both halves.
    pub fn split_at(&self, byte_index: usize) -> (LineLayout, LineLayout) {
        let x_offset = self.x_for_index(byte_index);

        // Partition glyph runs. A single run may contribute glyphs to both halves.
        let mut left_runs = Vec::new();
        let mut right_runs = Vec::new();

        for run in &self.runs {
            let split_pos = run.glyphs.partition_point(|g| g.index < byte_index);

            if split_pos > 0 {
                left_runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs: run.glyphs[..split_pos].to_vec(),
                });
            }

            if split_pos < run.glyphs.len() {
                let right_glyphs = run.glyphs[split_pos..]
                    .iter()
                    .map(|g| ShapedGlyph {
                        id: g.id,
                        position: point(g.position.x - x_offset, g.position.y),
                        index: g.index - byte_index,
                        is_emoji: g.is_emoji,
                    })
                    .collect();
                right_runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs: right_glyphs,
                });
            }
        }

        let left = LineLayout {
            font_size: self.font_size,
            width: x_offset,
            ascent: self.ascent,
            descent: self.descent,
            runs: left_runs,
            len: byte_index,
        };

        let right = LineLayout {
            font_size: self.font_size,
            width: self.width - x_offset,
            ascent: self.ascent,
            descent: self.descent,
            runs: right_runs,
            len: self.len - byte_index,
        };

        (left, right)
    }

    fn compute_wrap_boundaries(
        &self,
        text: &str,
        wrap_width: Pixels,
        max_lines: Option<usize>,
    ) -> SmallVec<[WrapBoundary; 1]> {
        let mut boundaries = SmallVec::new();
        let mut first_non_whitespace_ix = None;
        let mut last_candidate_ix = None;
        let mut last_candidate_x = px(0.);
        let mut last_boundary = WrapBoundary {
            run_ix: 0,
            glyph_ix: 0,
        };
        let mut last_boundary_x = px(0.);
        let mut prev_ch: Option<char> = None;
        let mut glyphs = self
            .runs
            .iter()
            .enumerate()
            .flat_map(move |(run_ix, run)| {
                run.glyphs.iter().enumerate().map(move |(glyph_ix, glyph)| {
                    let character = text[glyph.index..].chars().next().unwrap();
                    (
                        WrapBoundary { run_ix, glyph_ix },
                        character,
                        glyph.position.x,
                    )
                })
            })
            .peekable();

        while let Some((boundary, ch, x)) = glyphs.next() {
            if ch == '\n' {
                prev_ch = None;
                continue;
            }

            // This applies the same candidate rule as `LineWrapper::wrap_line`; the two walks
            // differ only in how widths are measured.
            if LineWrapper::is_break_candidate(prev_ch, ch, first_non_whitespace_ix.is_some()) {
                last_candidate_ix = Some(boundary);
                last_candidate_x = x;
            }

            if ch != ' ' && first_non_whitespace_ix.is_none() {
                first_non_whitespace_ix = Some(boundary);
            }

            let next_x = glyphs.peek().map_or(self.width, |(_, _, x)| *x);
            let width = next_x - last_boundary_x;

            if width > wrap_width && boundary > last_boundary {
                // When used line_clamp, we should limit the number of lines.
                if let Some(max_lines) = max_lines
                    && boundaries.len() >= max_lines.saturating_sub(1)
                {
                    break;
                }

                if let Some(last_candidate_ix) = last_candidate_ix.take() {
                    last_boundary = last_candidate_ix;
                    last_boundary_x = last_candidate_x;
                } else {
                    last_boundary = boundary;
                    last_boundary_x = x;
                }
                boundaries.push(last_boundary);
            }
            prev_ch = Some(ch);
        }

        boundaries
    }
}

/// A line of text that has been wrapped to fit a given width
#[derive(Default, Debug)]
pub struct WrappedLineLayout {
    /// The line layout, pre-wrapping.
    pub unwrapped_layout: Arc<LineLayout>,

    /// The boundaries at which the line was wrapped
    pub wrap_boundaries: SmallVec<[WrapBoundary; 1]>,

    /// The width of the line, if it was wrapped
    pub wrap_width: Option<Pixels>,
}

/// A boundary at which a line was wrapped
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct WrapBoundary {
    /// The index in the run just before the line was wrapped
    pub run_ix: usize,
    /// The index of the glyph just before the line was wrapped
    pub glyph_ix: usize,
}

impl WrappedLineLayout {
    /// The length of the underlying text, in utf8 bytes.
    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.unwrapped_layout.len
    }

    /// The width of this line, in pixels, whether or not it was wrapped.
    pub fn width(&self) -> Pixels {
        self.wrap_width
            .unwrap_or(Pixels::MAX)
            .min(self.unwrapped_layout.width)
    }

    /// The size of the whole wrapped text, for the given line_height.
    /// can span multiple lines if there are multiple wrap boundaries.
    pub fn size(&self, line_height: Pixels) -> Size<Pixels> {
        Size {
            width: self.width(),
            height: line_height * (self.wrap_boundaries.len() + 1),
        }
    }

    /// The ascent of a line in this layout
    pub fn ascent(&self) -> Pixels {
        self.unwrapped_layout.ascent
    }

    /// The descent of a line in this layout
    pub fn descent(&self) -> Pixels {
        self.unwrapped_layout.descent
    }

    /// The wrap boundaries in this layout
    pub fn wrap_boundaries(&self) -> &[WrapBoundary] {
        &self.wrap_boundaries
    }

    /// The font size of this layout
    pub fn font_size(&self) -> Pixels {
        self.unwrapped_layout.font_size
    }

    /// The runs in this layout, sans wrapping
    pub fn runs(&self) -> &[ShapedRun] {
        &self.unwrapped_layout.runs
    }

    /// The index corresponding to a given position in this layout for the given line height.
    ///
    /// See also [`Self::closest_index_for_position`].
    pub fn index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        self._index_for_position(position, line_height, false)
    }

    /// The closest index to a given position in this layout for the given line height.
    ///
    /// Closest means the character boundary closest to the given position.
    ///
    /// See also [`LineLayout::closest_index_for_x`].
    pub fn closest_index_for_position(
        &self,
        position: Point<Pixels>,
        line_height: Pixels,
    ) -> Result<usize, usize> {
        self._index_for_position(position, line_height, true)
    }

    fn _index_for_position(
        &self,
        mut position: Point<Pixels>,
        line_height: Pixels,
        closest: bool,
    ) -> Result<usize, usize> {
        let wrapped_line_ix = (position.y / line_height) as usize;

        let wrapped_line_start_index;
        let wrapped_line_start_x;
        if wrapped_line_ix > 0 {
            let Some(line_start_boundary) = self.wrap_boundaries.get(wrapped_line_ix - 1) else {
                return Err(0);
            };
            let run = &self.unwrapped_layout.runs[line_start_boundary.run_ix];
            let glyph = &run.glyphs[line_start_boundary.glyph_ix];
            wrapped_line_start_index = glyph.index;
            wrapped_line_start_x = glyph.position.x;
        } else {
            wrapped_line_start_index = 0;
            wrapped_line_start_x = Pixels::ZERO;
        };

        let wrapped_line_end_index;
        let wrapped_line_end_x;
        if wrapped_line_ix < self.wrap_boundaries.len() {
            let next_wrap_boundary_ix = wrapped_line_ix;
            let next_wrap_boundary = self.wrap_boundaries[next_wrap_boundary_ix];
            let run = &self.unwrapped_layout.runs[next_wrap_boundary.run_ix];
            let glyph = &run.glyphs[next_wrap_boundary.glyph_ix];
            wrapped_line_end_index = glyph.index;
            wrapped_line_end_x = glyph.position.x;
        } else {
            wrapped_line_end_index = self.unwrapped_layout.len;
            wrapped_line_end_x = self.unwrapped_layout.width;
        };

        let mut position_in_unwrapped_line = position;
        position_in_unwrapped_line.x += wrapped_line_start_x;
        if position_in_unwrapped_line.x < wrapped_line_start_x {
            Err(wrapped_line_start_index)
        } else if position_in_unwrapped_line.x >= wrapped_line_end_x {
            Err(wrapped_line_end_index)
        } else {
            if closest {
                Ok(self
                    .unwrapped_layout
                    .closest_index_for_x(position_in_unwrapped_line.x))
            } else {
                // The shaper can place a trailing zero-width wrap boundary glyph slightly past
                // the line's width, so the row can extend past where `index_for_x` has glyphs.
                self.unwrapped_layout
                    .index_for_x(position_in_unwrapped_line.x)
                    .ok_or(wrapped_line_end_index)
            }
        }
    }

    /// Returns the pixel position for the given byte index.
    pub fn position_for_index(&self, index: usize, line_height: Pixels) -> Option<Point<Pixels>> {
        let mut line_start_ix = 0;
        let mut line_end_indices = self
            .wrap_boundaries
            .iter()
            .map(|wrap_boundary| {
                let run = &self.unwrapped_layout.runs[wrap_boundary.run_ix];
                let glyph = &run.glyphs[wrap_boundary.glyph_ix];
                glyph.index
            })
            .chain([self.len()])
            .enumerate();
        for (ix, line_end_ix) in line_end_indices {
            let line_y = ix as f32 * line_height;
            if index < line_start_ix {
                break;
            } else if index > line_end_ix {
                line_start_ix = line_end_ix;
                continue;
            } else {
                let line_start_x = self.unwrapped_layout.x_for_index(line_start_ix);
                let x = self.unwrapped_layout.x_for_index(index) - line_start_x;
                return Some(point(x, line_y));
            }
        }

        None
    }
}

pub(crate) struct LineLayoutCache {
    previous_frame: Mutex<FrameCache>,
    current_frame: RwLock<FrameCache>,
    platform_text_system: Arc<dyn PlatformTextSystem>,
    /// Advances when [`TextSystem::add_fonts`] successfully changes the font database.
    font_generation: Arc<AtomicUsize>,
    /// Records the generation represented by both frame caches.
    cached_font_generation: AtomicUsize,
    /// Recent shapes and numeric shaping, shared with every window text system of the text system.
    shape_caches: Arc<ShapeCaches>,
}

#[derive(Default)]
struct FrameCache {
    lines: FxHashMap<Arc<CacheKey>, Arc<LineLayout>>,
    wrapped_lines: FxHashMap<Arc<CacheKey>, Arc<WrappedLineLayout>>,
    used_lines: Vec<Arc<CacheKey>>,
    used_wrapped_lines: Vec<Arc<CacheKey>>,

    // Content-addressable caches keyed by caller-provided text hash + layout params.
    // These allow cache hits without materializing a contiguous `SharedString`.
    //
    // IMPORTANT: To support allocation-free lookups, we store these maps using a key type
    // (`HashedCacheKeyRef`) that can be computed without building a contiguous `&str`/`SharedString`.
    // On miss, we allocate once and store under an owned `HashedCacheKey`.
    lines_by_hash: FxHashMap<Arc<HashedCacheKey>, Arc<LineLayout>>,
    wrapped_lines_by_hash: FxHashMap<Arc<HashedCacheKey>, Arc<WrappedLineLayout>>,
    used_lines_by_hash: Vec<Arc<HashedCacheKey>>,
    used_wrapped_lines_by_hash: Vec<Arc<HashedCacheKey>>,
}

#[derive(Clone, Default)]
pub(crate) struct LineLayoutIndex {
    font_generation: usize,
    lines_index: usize,
    wrapped_lines_index: usize,
    lines_by_hash_index: usize,
    wrapped_lines_by_hash_index: usize,
}

impl LineLayoutIndex {
    /// Whether `self` lies within `start..=end`.
    pub(crate) fn within(&self, start: &Self, end: &Self) -> bool {
        self.lines_index >= start.lines_index
            && self.lines_index <= end.lines_index
            && self.wrapped_lines_index >= start.wrapped_lines_index
            && self.wrapped_lines_index <= end.wrapped_lines_index
            && self.lines_by_hash_index >= start.lines_by_hash_index
            && self.lines_by_hash_index <= end.lines_by_hash_index
            && self.wrapped_lines_by_hash_index >= start.wrapped_lines_by_hash_index
            && self.wrapped_lines_by_hash_index <= end.wrapped_lines_by_hash_index
    }

    /// Moves an index recorded inside a range that began at `from` to the
    /// equivalent position inside a copy of that range that began at `to`.
    pub(crate) fn rebased(&self, from: &Self, to: &Self) -> Self {
        Self {
            font_generation: to.font_generation,
            lines_index: self.lines_index - from.lines_index + to.lines_index,
            wrapped_lines_index: self.wrapped_lines_index - from.wrapped_lines_index
                + to.wrapped_lines_index,
            lines_by_hash_index: self.lines_by_hash_index - from.lines_by_hash_index
                + to.lines_by_hash_index,
            wrapped_lines_by_hash_index: self.wrapped_lines_by_hash_index
                - from.wrapped_lines_by_hash_index
                + to.wrapped_lines_by_hash_index,
        }
    }

    /// Position in the used string-keyed line list.
    #[cfg(test)]
    pub(crate) fn lines_position(&self) -> usize {
        self.lines_index
    }

    fn of(font_generation: usize, frame: &FrameCache) -> Self {
        Self {
            font_generation,
            lines_index: frame.used_lines.len(),
            wrapped_lines_index: frame.used_wrapped_lines.len(),
            lines_by_hash_index: frame.used_lines_by_hash.len(),
            wrapped_lines_by_hash_index: frame.used_wrapped_lines_by_hash.len(),
        }
    }
}

/// Moves every entry of `other` into `kept`, keeping the larger map to minimize movement.
/// Entries already in `kept` (the finished frame) win over `other` (older survivors).
fn merge_into_larger<K, V>(kept: &mut FxHashMap<K, V>, other: &mut FxHashMap<K, V>)
where
    K: Eq + Hash,
{
    if other.len() > kept.len() {
        std::mem::swap(kept, other);
        // `other` now holds the finished frame's entries, which must win.
        for (key, value) in other.drain() {
            kept.insert(key, value);
        }
    } else {
        for (key, value) in other.drain() {
            kept.entry(key).or_insert(value);
        }
    }
}

impl LineLayoutCache {
    pub(crate) fn new(
        platform_text_system: Arc<dyn PlatformTextSystem>,
        font_generation: Arc<AtomicUsize>,
        shape_caches: Arc<ShapeCaches>,
    ) -> Self {
        let cached_font_generation = font_generation.load(Ordering::Acquire);
        Self {
            previous_frame: Mutex::default(),
            current_frame: RwLock::default(),
            platform_text_system,
            font_generation,
            cached_font_generation: AtomicUsize::new(cached_font_generation),
            shape_caches,
        }
    }

    /// Shapes a line that missed the frame cache: recent shapes first, then numeric assembly,
    /// then the platform. No cache lock is held while the platform shapes.
    fn shape_line(
        &self,
        font_generation: usize,
        text: &SharedString,
        font_size: Pixels,
        runs: &[FontRun],
    ) -> LineLayout {
        let caches = &self.shape_caches;
        if let Some(layout) = caches.recent_lookup(font_generation, text, font_size, runs) {
            return layout;
        }
        let layout = match caches.numeric_layout(
            self.platform_text_system.as_ref(),
            font_generation,
            text,
            font_size,
            runs,
        ) {
            NumericOutcome::Layout(layout) => layout,
            NumericOutcome::Platform => {
                caches.platform_shape(self.platform_text_system.as_ref(), text, font_size, runs)
            }
        };
        caches.recent_insert(font_generation, text, font_size, runs, &layout);
        layout
    }

    pub fn layout_index(&self) -> LineLayoutIndex {
        let font_generation = self.clear_if_font_generation_changed();
        let frame = self.current_frame.read();
        LineLayoutIndex::of(font_generation, &frame)
    }

    /// Replays the used-entry range of the previous frame into the current frame and returns the
    /// range the replayed entries occupy in the current frame (the range rebased to its
    /// destination coordinates, stamped with the current font generation). Replaying a range
    /// recorded under an earlier font generation reuses nothing and returns an empty range.
    pub fn reuse_layouts(&self, range: Range<LineLayoutIndex>) -> Range<LineLayoutIndex> {
        let font_generation = self.clear_if_font_generation_changed();
        let current_frame = &mut *self.current_frame.write();
        let previous_frame = &mut *self.previous_frame.lock();
        let start = LineLayoutIndex::of(font_generation, current_frame);
        if range.start.font_generation != font_generation
            || range.end.font_generation != font_generation
        {
            return start.clone()..start;
        }

        for key in &previous_frame.used_lines[range.start.lines_index..range.end.lines_index] {
            if let Some((key, line)) = previous_frame.lines.remove_entry(key) {
                current_frame.lines.insert(key, line);
            }
            current_frame.used_lines.push(key.clone());
        }

        for key in &previous_frame.used_wrapped_lines
            [range.start.wrapped_lines_index..range.end.wrapped_lines_index]
        {
            if let Some((key, line)) = previous_frame.wrapped_lines.remove_entry(key) {
                current_frame.wrapped_lines.insert(key, line);
            }
            current_frame.used_wrapped_lines.push(key.clone());
        }

        for key in &previous_frame.used_lines_by_hash
            [range.start.lines_by_hash_index..range.end.lines_by_hash_index]
        {
            if let Some((key, line)) = previous_frame.lines_by_hash.remove_entry(key) {
                current_frame.lines_by_hash.insert(key, line);
            }
            current_frame.used_lines_by_hash.push(key.clone());
        }

        for key in &previous_frame.used_wrapped_lines_by_hash
            [range.start.wrapped_lines_by_hash_index..range.end.wrapped_lines_by_hash_index]
        {
            if let Some((key, line)) = previous_frame.wrapped_lines_by_hash.remove_entry(key) {
                current_frame.wrapped_lines_by_hash.insert(key, line);
            }
            current_frame.used_wrapped_lines_by_hash.push(key.clone());
        }

        let end = LineLayoutIndex::of(font_generation, current_frame);
        start..end
    }

    pub fn truncate_layouts(&self, index: LineLayoutIndex) {
        let font_generation = self.clear_if_font_generation_changed();
        if index.font_generation != font_generation {
            return;
        }
        let mut current_frame = &mut *self.current_frame.write();
        current_frame.used_lines.truncate(index.lines_index);
        current_frame
            .used_wrapped_lines
            .truncate(index.wrapped_lines_index);
        current_frame
            .used_lines_by_hash
            .truncate(index.lines_by_hash_index);
        current_frame
            .used_wrapped_lines_by_hash
            .truncate(index.wrapped_lines_by_hash_index);
    }

    /// Rolls the frame caches over. Layouts of the previous frame that the finishing frame did
    /// not look up but that something else still holds (a measurement, a retained element) stay
    /// cached; everything else leaves.
    pub fn finish_frame(&self) {
        let _font_generation = self.clear_if_font_generation_changed();
        let mut curr_frame = self.current_frame.write();
        let mut prev_frame = self.previous_frame.lock();
        // The finished frame becomes the previous one; `curr_frame` now holds the entries the
        // finished frame did not look up.
        std::mem::swap(&mut *prev_frame, &mut *curr_frame);
        curr_frame.used_lines.clear();
        curr_frame.used_wrapped_lines.clear();
        curr_frame.used_lines_by_hash.clear();
        curr_frame.used_wrapped_lines_by_hash.clear();

        // A wrapped layout holds the unwrapped line it was wrapped from, so wrappers go first:
        // releasing one can leave its line unreferenced.
        curr_frame
            .wrapped_lines
            .retain(|_, layout| Arc::strong_count(layout) > 1);
        curr_frame
            .wrapped_lines_by_hash
            .retain(|_, layout| Arc::strong_count(layout) > 1);
        curr_frame
            .lines
            .retain(|_, layout| Arc::strong_count(layout) > 1);
        curr_frame
            .lines_by_hash
            .retain(|_, layout| Arc::strong_count(layout) > 1);

        merge_into_larger(&mut prev_frame.lines, &mut curr_frame.lines);
        merge_into_larger(&mut prev_frame.wrapped_lines, &mut curr_frame.wrapped_lines);
        merge_into_larger(&mut prev_frame.lines_by_hash, &mut curr_frame.lines_by_hash);
        merge_into_larger(
            &mut prev_frame.wrapped_lines_by_hash,
            &mut curr_frame.wrapped_lines_by_hash,
        );
    }

    pub fn layout_wrapped_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        wrap_width: Option<Pixels>,
        max_lines: Option<usize>,
    ) -> Arc<WrappedLineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let _font_generation = self.clear_if_font_generation_changed();
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width,
            force_width: None,
        } as &dyn AsCacheKeyRef;

        let current_frame = self.current_frame.upgradable_read();
        if let Some(layout) = current_frame.wrapped_lines.get(key) {
            return layout.clone();
        }

        let previous_frame_entry = self.previous_frame.lock().wrapped_lines.remove_entry(key);
        if let Some((key, layout)) = previous_frame_entry {
            let mut current_frame = RwLockUpgradableReadGuard::upgrade(current_frame);
            current_frame
                .wrapped_lines
                .insert(key.clone(), layout.clone());
            current_frame.used_wrapped_lines.push(key);
            layout
        } else {
            drop(current_frame);
            let text = SharedString::from(text);
            let unwrapped_layout = self.layout_line::<&SharedString>(&text, font_size, runs, None);
            let wrap_boundaries = if let Some(wrap_width) = wrap_width {
                unwrapped_layout.compute_wrap_boundaries(text.as_ref(), wrap_width, max_lines)
            } else {
                SmallVec::new()
            };
            let layout = Arc::new(WrappedLineLayout {
                unwrapped_layout,
                wrap_boundaries,
                wrap_width,
            });
            let key = Arc::new(CacheKey {
                text,
                font_size,
                runs: SmallVec::from(runs),
                wrap_width,
                force_width: None,
            });

            let mut current_frame = self.current_frame.write();
            current_frame
                .wrapped_lines
                .insert(key.clone(), layout.clone());
            current_frame.used_wrapped_lines.push(key);

            layout
        }
    }

    pub fn layout_line<Text>(
        &self,
        text: Text,
        font_size: Pixels,
        runs: &[FontRun],
        force_width: Option<Pixels>,
    ) -> Arc<LineLayout>
    where
        Text: AsRef<str>,
        SharedString: From<Text>,
    {
        let font_generation = self.clear_if_font_generation_changed();
        let key = &CacheKeyRef {
            text: text.as_ref(),
            font_size,
            runs,
            wrap_width: None,
            force_width,
        } as &dyn AsCacheKeyRef;

        let current_frame = self.current_frame.upgradable_read();
        if let Some(layout) = current_frame.lines.get(key) {
            return layout.clone();
        }

        let mut current_frame = RwLockUpgradableReadGuard::upgrade(current_frame);
        if let Some((key, layout)) = self.previous_frame.lock().lines.remove_entry(key) {
            current_frame.lines.insert(key.clone(), layout.clone());
            current_frame.used_lines.push(key);
            layout
        } else {
            let text = SharedString::from(text);
            let mut layout = self.shape_line(font_generation, &text, font_size, runs);

            if let Some(force_width) = force_width {
                apply_force_width_to_layout(&mut layout, force_width);
            }

            let key = Arc::new(CacheKey {
                text,
                font_size,
                runs: SmallVec::from(runs),
                wrap_width: None,
                force_width,
            });
            let layout = Arc::new(layout);
            current_frame.lines.insert(key.clone(), layout.clone());
            current_frame.used_lines.push(key);
            layout
        }
    }

    /// Try to retrieve a previously-shaped line layout using a caller-provided content hash.
    ///
    /// This is a *non-allocating* cache probe: it does not materialize any text. If the layout
    /// is not already cached in either the current frame or previous frame, returns `None`.
    ///
    /// Contract (caller enforced):
    /// - Same `text_hash` implies identical text content (collision risk accepted by caller).
    /// - `text_len` should be the UTF-8 byte length of the text (helps reduce accidental collisions).
    pub fn try_layout_line_by_hash(
        &self,
        text_hash: u64,
        text_len: usize,
        font_size: Pixels,
        runs: &[FontRun],
        force_width: Option<Pixels>,
    ) -> Option<Arc<LineLayout>> {
        let _font_generation = self.clear_if_font_generation_changed();
        let key_ref = HashedCacheKeyRef {
            text_hash,
            text_len,
            font_size,
            runs,
            wrap_width: None,
            force_width,
        };

        let current_frame = self.current_frame.read();
        if let Some((_, layout)) = current_frame.lines_by_hash.iter().find(|(key, _)| {
            HashedCacheKeyRef {
                text_hash: key.text_hash,
                text_len: key.text_len,
                font_size: key.font_size,
                runs: key.runs.as_slice(),
                wrap_width: key.wrap_width,
                force_width: key.force_width,
            } == key_ref
        }) {
            return Some(layout.clone());
        }

        let previous_frame = self.previous_frame.lock();
        if let Some((_, layout)) = previous_frame.lines_by_hash.iter().find(|(key, _)| {
            HashedCacheKeyRef {
                text_hash: key.text_hash,
                text_len: key.text_len,
                font_size: key.font_size,
                runs: key.runs.as_slice(),
                wrap_width: key.wrap_width,
                force_width: key.force_width,
            } == key_ref
        }) {
            return Some(layout.clone());
        }

        None
    }

    /// Layout a line of text using a caller-provided content hash as the cache key.
    ///
    /// This enables cache hits without materializing a contiguous `SharedString` for `text`.
    /// If the cache misses, `materialize_text` is invoked to produce the `SharedString` for shaping.
    ///
    /// Contract (caller enforced):
    /// - Same `text_hash` implies identical text content (collision risk accepted by caller).
    /// - `text_len` should be the UTF-8 byte length of the text (helps reduce accidental collisions).
    pub fn layout_line_by_hash(
        &self,
        text_hash: u64,
        text_len: usize,
        font_size: Pixels,
        runs: &[FontRun],
        force_width: Option<Pixels>,
        materialize_text: impl FnOnce() -> SharedString,
    ) -> Arc<LineLayout> {
        let font_generation = self.clear_if_font_generation_changed();
        let key_ref = HashedCacheKeyRef {
            text_hash,
            text_len,
            font_size,
            runs,
            wrap_width: None,
            force_width,
        };

        // Fast path: already cached (no allocation).
        let current_frame = self.current_frame.upgradable_read();
        if let Some((_, layout)) = current_frame.lines_by_hash.iter().find(|(key, _)| {
            HashedCacheKeyRef {
                text_hash: key.text_hash,
                text_len: key.text_len,
                font_size: key.font_size,
                runs: key.runs.as_slice(),
                wrap_width: key.wrap_width,
                force_width: key.force_width,
            } == key_ref
        }) {
            return layout.clone();
        }

        let mut current_frame = RwLockUpgradableReadGuard::upgrade(current_frame);

        // Try to reuse from previous frame without allocating; do a linear scan to find a matching key.
        // (We avoid `drain()` here because it would eagerly move all entries.)
        let mut previous_frame = self.previous_frame.lock();
        if let Some(existing_key) = previous_frame
            .used_lines_by_hash
            .iter()
            .find(|key| {
                HashedCacheKeyRef {
                    text_hash: key.text_hash,
                    text_len: key.text_len,
                    font_size: key.font_size,
                    runs: key.runs.as_slice(),
                    wrap_width: key.wrap_width,
                    force_width: key.force_width,
                } == key_ref
            })
            .cloned()
        {
            if let Some((key, layout)) = previous_frame.lines_by_hash.remove_entry(&existing_key) {
                current_frame
                    .lines_by_hash
                    .insert(key.clone(), layout.clone());
                current_frame.used_lines_by_hash.push(key);
                return layout;
            }
        }

        let text = materialize_text();
        let mut layout = self.shape_line(font_generation, &text, font_size, runs);

        if let Some(force_width) = force_width {
            apply_force_width_to_layout(&mut layout, force_width);
        }

        let key = Arc::new(HashedCacheKey {
            text_hash,
            text_len,
            font_size,
            runs: SmallVec::from(runs),
            wrap_width: None,
            force_width,
        });
        let layout = Arc::new(layout);
        current_frame
            .lines_by_hash
            .insert(key.clone(), layout.clone());
        current_frame.used_lines_by_hash.push(key);
        layout
    }

    /// Entry counts of the frame caches: `[current lines, current wrapped lines, previous lines,
    /// previous wrapped lines]` (string-keyed and hash-keyed entries together).
    #[cfg(test)]
    pub(crate) fn cached_entry_counts(&self) -> [usize; 4] {
        let current = self.current_frame.read();
        let previous = self.previous_frame.lock();
        [
            current.lines.len() + current.lines_by_hash.len(),
            current.wrapped_lines.len() + current.wrapped_lines_by_hash.len(),
            previous.lines.len() + previous.lines_by_hash.len(),
            previous.wrapped_lines.len() + previous.wrapped_lines_by_hash.len(),
        ]
    }

    /// Used-entry list lengths of the current frame.
    #[cfg(test)]
    pub(crate) fn used_entry_counts(&self) -> [usize; 4] {
        let current = self.current_frame.read();
        [
            current.used_lines.len(),
            current.used_wrapped_lines.len(),
            current.used_lines_by_hash.len(),
            current.used_wrapped_lines_by_hash.len(),
        ]
    }

    fn clear_if_font_generation_changed(&self) -> usize {
        let font_generation = self.font_generation.load(Ordering::Acquire);
        if self.cached_font_generation.load(Ordering::Acquire) == font_generation {
            return font_generation;
        }

        let mut current_frame = self.current_frame.write();
        if self.cached_font_generation.load(Ordering::Acquire) == font_generation {
            return font_generation;
        }

        *current_frame = FrameCache::default();
        *self.previous_frame.lock() = FrameCache::default();
        self.cached_font_generation
            .store(font_generation, Ordering::Release);
        font_generation
    }
}

// Combining marks (e.g. Thai vowel signs, Arabic diacritics) are shaped by
// HarfBuzz at the same x position as their base character. The force-width
// loop must not advance the cell counter for these zero-advance glyphs,
// otherwise they get displaced into the next cell. We detect them by checking
// whether shaped x has advanced by at least half a cell beyond the last base.
fn apply_force_width_to_layout(layout: &mut LineLayout, force_width: Pixels) {
    let mut glyph_pos: usize = 0;
    // NEG_INFINITY ensures the first glyph is always classified as a base.
    let mut last_base_shaped_x = px(f32::NEG_INFINITY);
    let mut last_base_actual_x = px(0.);

    for run in layout.runs.iter_mut() {
        for glyph in run.glyphs.iter_mut() {
            let shaped_x = glyph.position.x;

            if shaped_x > last_base_shaped_x + force_width * 0.5 {
                let forced_x = glyph_pos * force_width;
                if (shaped_x - forced_x).abs() > px(1.) {
                    glyph.position.x = forced_x;
                }
                last_base_shaped_x = shaped_x;
                last_base_actual_x = glyph.position.x;
                glyph_pos += 1;
            } else {
                glyph.position.x = last_base_actual_x + (shaped_x - last_base_shaped_x);
            }
        }
    }
}

/// A run of text with a single font.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[expect(missing_docs)]
pub struct FontRun {
    pub len: usize,
    pub font_id: FontId,
}

trait AsCacheKeyRef {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_>;
}

#[derive(Clone, Debug, Eq)]
struct CacheKey {
    text: SharedString,
    font_size: Pixels,
    runs: SmallVec<[FontRun; 1]>,
    wrap_width: Option<Pixels>,
    force_width: Option<Pixels>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct CacheKeyRef<'a> {
    text: &'a str,
    font_size: Pixels,
    runs: &'a [FontRun],
    wrap_width: Option<Pixels>,
    force_width: Option<Pixels>,
}

#[derive(Clone, Debug)]
struct HashedCacheKey {
    text_hash: u64,
    text_len: usize,
    font_size: Pixels,
    runs: SmallVec<[FontRun; 1]>,
    wrap_width: Option<Pixels>,
    force_width: Option<Pixels>,
}

#[derive(Copy, Clone)]
struct HashedCacheKeyRef<'a> {
    text_hash: u64,
    text_len: usize,
    font_size: Pixels,
    runs: &'a [FontRun],
    wrap_width: Option<Pixels>,
    force_width: Option<Pixels>,
}

impl PartialEq for dyn AsCacheKeyRef + '_ {
    fn eq(&self, other: &dyn AsCacheKeyRef) -> bool {
        self.as_cache_key_ref() == other.as_cache_key_ref()
    }
}

impl PartialEq for HashedCacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.text_hash == other.text_hash
            && self.text_len == other.text_len
            && self.font_size == other.font_size
            && self.runs.as_slice() == other.runs.as_slice()
            && self.wrap_width == other.wrap_width
            && self.force_width == other.force_width
    }
}

impl Eq for HashedCacheKey {}

impl Hash for HashedCacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text_hash.hash(state);
        self.text_len.hash(state);
        self.font_size.hash(state);
        self.runs.as_slice().hash(state);
        self.wrap_width.hash(state);
        self.force_width.hash(state);
    }
}

impl PartialEq for HashedCacheKeyRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.text_hash == other.text_hash
            && self.text_len == other.text_len
            && self.font_size == other.font_size
            && self.runs == other.runs
            && self.wrap_width == other.wrap_width
            && self.force_width == other.force_width
    }
}

impl Eq for HashedCacheKeyRef<'_> {}

impl Hash for HashedCacheKeyRef<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text_hash.hash(state);
        self.text_len.hash(state);
        self.font_size.hash(state);
        self.runs.hash(state);
        self.wrap_width.hash(state);
        self.force_width.hash(state);
    }
}

impl Eq for dyn AsCacheKeyRef + '_ {}

impl Hash for dyn AsCacheKeyRef + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state)
    }
}

impl AsCacheKeyRef for CacheKey {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        CacheKeyRef {
            text: &self.text,
            font_size: self.font_size,
            runs: self.runs.as_slice(),
            wrap_width: self.wrap_width,
            force_width: self.force_width,
        }
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        self.as_cache_key_ref().eq(&other.as_cache_key_ref())
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_cache_key_ref().hash(state);
    }
}

impl<'a> Borrow<dyn AsCacheKeyRef + 'a> for Arc<CacheKey> {
    fn borrow(&self) -> &(dyn AsCacheKeyRef + 'a) {
        self.as_ref() as &dyn AsCacheKeyRef
    }
}

impl AsCacheKeyRef for CacheKeyRef<'_> {
    fn as_cache_key_ref(&self) -> CacheKeyRef<'_> {
        *self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GlyphId;

    fn glyph_at(x: f32, index: usize) -> ShapedGlyph {
        ShapedGlyph {
            id: GlyphId(0),
            position: point(px(x), px(0.)),
            index,
            is_emoji: false,
        }
    }

    fn make_layout(glyphs: Vec<ShapedGlyph>) -> LineLayout {
        LineLayout {
            font_size: px(16.),
            width: px(100.),
            ascent: px(12.),
            descent: px(4.),
            runs: vec![ShapedRun {
                font_id: FontId(0),
                glyphs,
            }],
            len: 0,
        }
    }

    fn glyph_x_positions(layout: &LineLayout) -> Vec<f32> {
        layout.runs[0]
            .glyphs
            .iter()
            .map(|g| f32::from(g.position.x))
            .collect()
    }

    #[test]
    fn test_force_width_latin_unchanged() {
        let cell_width = px(8.);
        let mut layout = make_layout(vec![glyph_at(0., 0), glyph_at(8., 1), glyph_at(16., 2)]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0., 8., 16.]);
    }

    #[test]
    fn test_force_width_combining_marks_not_advanced() {
        let cell_width = px(8.);
        // Simulates Thai "กี" — base consonant at x=0, combining vowel also at x=0
        let mut layout = make_layout(vec![
            glyph_at(0., 0), // ก (base)
            glyph_at(0., 3), // ี (combining mark, same x)
        ]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0., 0.]);
    }

    #[test]
    fn test_force_width_base_after_combining_mark() {
        let cell_width = px(8.);
        let mut layout = make_layout(vec![glyph_at(0., 0), glyph_at(0., 3), glyph_at(8., 6)]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0., 0., 8.]);
    }

    #[test]
    fn test_force_width_multiple_combining_marks() {
        let cell_width = px(8.);
        // Simulates "ก้" — base + vowel + tone mark (two combining marks stacked)
        let mut layout = make_layout(vec![
            glyph_at(0., 0), // ก (base)
            glyph_at(0., 3), // vowel (combining)
            glyph_at(0., 6), // tone mark (combining)
            glyph_at(8., 9), // next base
        ]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0., 0., 0., 8.]);
    }

    #[test]
    fn test_force_width_corrects_drifted_base_positions() {
        let cell_width = px(8.);
        // Font metrics don't perfectly match cell grid — glyphs drift >1px from cell boundary
        let mut layout = make_layout(vec![
            glyph_at(0.5, 0),  // within 1px tolerance, kept as-is
            glyph_at(10.2, 1), // >1px off from 8.0, corrected
            glyph_at(19.8, 2), // >1px off from 16.0, corrected
        ]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0.5, 8., 16.]);
    }

    #[test]
    fn test_force_width_combining_mark_after_within_tolerance_base() {
        let cell_width = px(8.);
        // Base glyph is within 1px of grid so it keeps its shaped position.
        // The combining mark must align to the base's actual position, not the grid slot.
        let mut layout = make_layout(vec![glyph_at(0.5, 0), glyph_at(0.5, 3)]);

        apply_force_width_to_layout(&mut layout, cell_width);

        let positions = glyph_x_positions(&layout);
        assert_eq!(positions, vec![0.5, 0.5]);
    }
}

#[cfg(test)]
mod wrap_candidate_tests {
    use super::*;
    use crate::{
        Bounds, DevicePixels, Font, FontMetrics, IndentAdjustment, LineFragment, RenderGlyphParams,
        Result, TextRenderingMode, TextSystem, TruncateFrom, size,
    };
    use std::borrow::Cow;

    /// A platform text system in which every character advances by the same width, regardless
    /// of the character and of the font size (the "fixed-advance shaper").
    struct FixedAdvanceTextSystem {
        advance: Pixels,
    }

    impl PlatformTextSystem for FixedAdvanceTextSystem {
        fn add_fonts(&self, _fonts: Vec<Cow<'static, [u8]>>) -> Result<()> {
            Ok(())
        }

        fn all_font_names(&self) -> Vec<String> {
            Vec::new()
        }

        fn font_id(&self, _descriptor: &Font) -> Result<FontId> {
            Ok(FontId(0))
        }

        fn font_metrics(&self, _font_id: FontId) -> FontMetrics {
            FontMetrics {
                units_per_em: 1000,
                ascent: 1000.0,
                descent: -250.0,
                line_gap: 0.0,
                underline_position: -100.0,
                underline_thickness: 50.0,
                cap_height: 700.0,
                x_height: 500.0,
                bounding_box: Bounds {
                    origin: point(0.0, -250.0),
                    size: size(1000.0, 1250.0),
                },
            }
        }

        fn typographic_bounds(&self, _font_id: FontId, _glyph_id: GlyphId) -> Result<Bounds<f32>> {
            Ok(Bounds {
                origin: point(0.0, 0.0),
                size: size(1000.0, 1000.0),
            })
        }

        fn advance(&self, _font_id: FontId, _glyph_id: GlyphId) -> Result<Size<f32>> {
            Ok(size(self.advance.0, 0.0))
        }

        fn glyph_for_char(&self, _font_id: FontId, _ch: char) -> Option<GlyphId> {
            Some(GlyphId(0))
        }

        fn glyph_raster_bounds(&self, _params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
            Ok(Default::default())
        }

        fn rasterize_glyph(
            &self,
            _params: &RenderGlyphParams,
            raster_bounds: Bounds<DevicePixels>,
        ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
            Ok((raster_bounds.size, Vec::new()))
        }

        fn layout_line(&self, text: &str, font_size: Pixels, _runs: &[FontRun]) -> LineLayout {
            let glyphs: Vec<ShapedGlyph> = text
                .char_indices()
                .enumerate()
                .map(|(column, (index, _))| ShapedGlyph {
                    id: GlyphId(0),
                    position: point(self.advance * column as f32, px(0.)),
                    index,
                    is_emoji: false,
                })
                .collect();
            let width = self.advance * glyphs.len() as f32;
            let runs = if glyphs.is_empty() {
                Vec::new()
            } else {
                vec![ShapedRun {
                    font_id: FontId(0),
                    glyphs,
                }]
            };
            LineLayout {
                font_size,
                width,
                ascent: font_size,
                descent: font_size * 0.25,
                runs,
                len: text.len(),
            }
        }

        fn recommended_rendering_mode(
            &self,
            _font_id: FontId,
            _font_size: Pixels,
        ) -> TextRenderingMode {
            TextRenderingMode::Grayscale
        }
    }

    const CELL: f32 = 8.;

    fn fixed_text_system(cell: f32) -> Arc<TextSystem> {
        Arc::new(TextSystem::new(Arc::new(FixedAdvanceTextSystem {
            advance: px(cell),
        })))
    }

    fn char_index(text: &str, byte_ix: usize) -> usize {
        text[..byte_ix].chars().count()
    }

    /// Boundaries (as character indices) chosen by the fragment wrapper.
    fn fragment_boundaries(text: &str, width: f32, cell: f32) -> Vec<usize> {
        let mut wrapper = LineWrapper::new(FontId(0), px(16.), fixed_text_system(cell));
        let fragments = [LineFragment::text(text)];
        let boundaries: Vec<usize> = wrapper
            .wrap_line(&fragments, px(width), IndentAdjustment::NoIndent)
            .map(|boundary| char_index(text, boundary.ix))
            .collect();
        boundaries
    }

    /// Boundaries (as character indices) chosen by the shaped-line wrap computation.
    fn shaped_boundaries(text: &str, width: f32, cell: f32) -> Vec<usize> {
        let layout = FixedAdvanceTextSystem { advance: px(cell) }.layout_line(text, px(16.), &[]);
        layout
            .compute_wrap_boundaries(text, px(width), None)
            .iter()
            .map(|boundary| {
                char_index(
                    text,
                    layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index,
                )
            })
            .collect()
    }

    /// Asserts that both wrapping paths break `text` exactly at `expected` (character indices).
    fn assert_wraps_at(text: &str, width: f32, expected: &[usize]) {
        assert_eq!(
            fragment_boundaries(text, width, CELL),
            expected,
            "fragment wrapper, {text:?} at width {width}"
        );
        assert_eq!(
            shaped_boundaries(text, width, CELL),
            expected,
            "shaped-line wrap boundaries, {text:?} at width {width}"
        );
    }

    /// Wraps with both paths (asserting they agree) and returns the resulting lines.
    fn wrapped_lines(text: &str, width: f32, cell: f32) -> Vec<String> {
        let from_fragments = fragment_boundaries(text, width, cell);
        let from_shaped = shaped_boundaries(text, width, cell);
        assert_eq!(
            from_fragments, from_shaped,
            "the two paths disagree for {text:?} at width {width}"
        );
        let chars: Vec<char> = text.chars().collect();
        let mut lines = Vec::new();
        let mut start = 0;
        for boundary in from_fragments {
            lines.push(chars[start..boundary].iter().collect());
            start = boundary;
        }
        lines.push(chars[start..].iter().collect());
        lines
    }

    #[test]
    fn a1_closing_quote_stays_with_its_word() {
        assert_wraps_at("aaa aaaa.\"", 72., &[4]);
    }

    #[test]
    fn a2_latin_openers_and_adjacent_cjk() {
        assert_wraps_at("aaa aaaaa!", 72., &[4]);
        assert_wraps_at("aaa (aaaa)", 72., &[4]);
        assert_wraps_at("aaaa \"bbbb", 72., &[5]);
        assert_wraps_at("今天的天气非常好我们去公", 72., &[9]);
    }

    #[test]
    fn a3_comma_and_question_mark_never_start_a_line() {
        assert_wraps_at("今天的天气非常好，我们", 68., &[7]);
        assert_wraps_at("今天的天气非常好,我们", 68., &[7]);
        assert_wraps_at("今天的天气非常好?我们", 68., &[7]);
        assert_wraps_at("aaa aaaaa,", 72., &[4]);
    }

    #[test]
    fn a4_prohibited_line_start_characters() {
        let prohibited_line_start = [
            '\u{3001}', '\u{3002}', '\u{FF0C}', '\u{FF0E}', '\u{FF1A}', '\u{FF1B}', '\u{FF1F}',
            '\u{FF01}', '\u{FF61}', '\u{FF64}', '\u{FF09}', '\u{FF3D}', '\u{FF5D}', '\u{300D}',
            '\u{300F}', '\u{3011}', '\u{3009}', '\u{300B}', '\u{3015}', '\u{3017}', '\u{3019}',
            '\u{301B}', '\u{301E}', '\u{301F}', '\u{FF60}', '\u{FF63}', '\u{2019}', '\u{201D}',
            '\u{2026}', '\u{2025}', '\u{30FB}', '\u{FF65}', '\u{301C}', '\u{203C}', '\u{2047}',
            '\u{2048}', '\u{2049}', '\u{3005}', '\u{303B}', '\u{309D}', '\u{309E}', '\u{30FD}',
            '\u{30FE}', '\u{FF05}',
        ];
        // Control: an ordinary ninth character begins the second line.
        assert_wraps_at("一二三四五六七八九", 64., &[8]);
        for s in prohibited_line_start {
            // Eight cells fit; `s` would begin the second line, so the break moves one earlier.
            let text = format!("一二三四五六七八{s}");
            assert_wraps_at(&text, 64., &[7]);
        }
    }

    #[test]
    fn a4_prohibited_line_end_characters() {
        let prohibited_line_end = [
            '(', '[', '{', '\u{FF08}', '\u{FF3B}', '\u{FF5B}', '\u{300C}', '\u{300E}', '\u{3010}',
            '\u{3008}', '\u{300A}', '\u{3014}', '\u{3016}', '\u{3018}', '\u{301A}', '\u{301D}',
            '\u{FF5F}', '\u{FF62}', '\u{2018}', '\u{201C}',
        ];
        // Control: a character that may end a line stays on the first line.
        assert_wraps_at("一二三四五六七八一", 64., &[8]);
        for e in prohibited_line_end {
            // `e` is the eighth (last fitting) cell and may not end the line.
            let text = format!("一二三四五六七{e}一");
            // U+2018 is a word character, so no candidate precedes it directly after CJK text
            // and the break falls before the preceding character instead.
            let expected: &[usize] = if e == '\u{2018}' { &[6] } else { &[7] };
            assert_wraps_at(&text, 64., expected);
        }
    }

    #[test]
    fn a4_runs_of_prohibited_characters_still_progress() {
        assert_wraps_at("((((((((((", 36., &[4, 8]);
        assert_wraps_at("、、、、、、、、、、", 36., &[4, 8]);
    }

    #[test]
    fn a5_paths_wrap_before_slashes() {
        assert_wraps_at("aa/bb/cc/dd", 36., &[2, 5, 8]);
    }

    #[test]
    fn a6_em_dash_pair_is_never_split() {
        let pair = "一二三四五六七\u{2014}\u{2014}八";
        assert_wraps_at(pair, 68., &[7]);
        let single = "一二三四五六七\u{2014}八九";
        assert_wraps_at(single, 68., &[8]);
    }

    #[test]
    fn a7_inline_element_clears_previous_character() {
        // The rule itself: an opening bracket holds on to the next character, unless an inline
        // element separates them (the element clears the previous-character context).
        assert!(!LineWrapper::is_break_candidate(Some('（'), '我', true));
        assert!(LineWrapper::is_break_candidate(None, '我', true));

        let fragments = [
            LineFragment::text("看（"),
            LineFragment::element(px(8.), 1),
            LineFragment::text("我们一起散步"),
        ];
        let mut wrapper = LineWrapper::new(FontId(0), px(16.), fixed_text_system(CELL));
        let boundaries: Vec<usize> = wrapper
            .wrap_line(&fragments, px(28.), IndentAdjustment::NoIndent)
            .map(|boundary| boundary.ix)
            .collect();
        // 看 and （ are three bytes each and the element is one byte, so index 7 is the
        // character right after the element (without the reset it would be 3, before （).
        assert_eq!(boundaries, vec![7, 16]);
    }

    #[test]
    fn a8_ascii_question_mark_after_cjk_range_boundaries() {
        let cases = [
            ('\u{2FFF}', false),
            ('\u{3000}', true),
            ('\u{30FF}', true),
            ('\u{3400}', true),
            ('\u{4DBF}', true),
            ('\u{4E00}', true),
            ('\u{9FFF}', true),
            ('\u{AC00}', true),
            ('\u{D7AF}', true),
            ('\u{F900}', true),
            ('\u{FAFF}', true),
            ('\u{FF00}', true),
            ('\u{FFEF}', true),
            ('\u{20000}', true),
        ];
        for (before, is_cjk) in cases {
            // The question mark overflows the line; it is kept with a CJK character (the break
            // moves before that character) but may start a line after any other character.
            let text = format!("一一一一一一一{before}?");
            let expected: &[usize] = if is_cjk { &[7] } else { &[8] };
            assert_wraps_at(&text, 64., expected);
        }
        assert_wraps_at("一一一一一一一a?", 64., &[8]);
    }

    #[test]
    fn a9_wrapped_truncation_uses_the_same_candidates() {
        let cases = [
            (
                "今天的天气非常好，我们一起去公园散步吧",
                68.,
                "今天的天气非常好，我们一起去…",
            ),
            (
                "一二三四五六七（八九十甲乙丙丁戊己",
                64.,
                "一二三四五六七（八九十甲乙丙…",
            ),
        ];
        for (text, width, expected) in cases {
            // Wrapping puts the first break before the comma's/bracket's holder, i.e. at 7...
            assert_eq!(fragment_boundaries(text, width, CELL)[0], 7);
            assert_eq!(shaped_boundaries(text, width, CELL)[0], 7);

            // ...so the second (last allowed) line starts there and holds seven cells.
            let mut wrapper = LineWrapper::new(FontId(0), px(16.), fixed_text_system(CELL));
            let (truncated, _) = wrapper.truncate_wrapped_line(
                text.into(),
                px(width),
                2,
                "…",
                &[],
                TruncateFrom::End,
            );
            assert_eq!(truncated.as_ref(), expected);
        }
    }

    #[test]
    fn a10_mixed_latin_and_cjk() {
        assert_wraps_at("Hello world你好世界", 100., &[12]);
        assert_wraps_at("Hello world，", 88., &[6]);
    }

    #[test]
    fn fid_txt_001_1_latin_expected_lines() {
        assert_eq!(wrapped_lines("aaa aaaa.\"", 76., 8.), ["aaa ", "aaaa.\""]);
        assert_eq!(wrapped_lines("aaa aaaa.\"", 140., 8.), ["aaa aaaa.\""]);
        assert_eq!(wrapped_lines("aaa aaaaa!", 76., 8.), ["aaa ", "aaaaa!"]);
        assert_eq!(wrapped_lines("aaa (aaaa)", 76., 8.), ["aaa ", "(aaaa)"]);
        assert_eq!(wrapped_lines("aaaa \"bbbb", 76., 8.), ["aaaa ", "\"bbbb"]);
    }

    #[test]
    fn fid_txt_001_2_cjk_expected_lines() {
        const T1: &str = "今天的天气非常好，我们一起去公园散步吧。";
        const T2: &str = "我用（新方法）重写了程序：速度提升了８％，超出预期……";
        const T3: &str = "好吗？我们走吧。「新」方法真好！";

        assert_eq!(
            wrapped_lines(T1, 170., 20.),
            ["今天的天气非常", "好，我们一起去公", "园散步吧。"]
        );
        assert_eq!(
            wrapped_lines(T2, 70., 20.),
            [
                "我用",
                "（新方",
                "法）重",
                "写了程",
                "序：速",
                "度提升",
                "了",
                "８％，",
                "超出预",
                "期……"
            ]
        );
        assert_eq!(
            wrapped_lines(T2, 130., 20.),
            [
                "我用（新方",
                "法）重写了程",
                "序：速度提升",
                "了８％，超出",
                "预期……"
            ]
        );
        assert_eq!(
            wrapped_lines(T2, 110., 20.),
            [
                "我用（新方",
                "法）重写了",
                "程序：速度",
                "提升了",
                "８％，超出",
                "预期……"
            ]
        );
        assert_eq!(
            wrapped_lines(T3, 110., 20.),
            ["好吗？我们", "走吧。", "「新」方法", "真好！"]
        );
    }
}
