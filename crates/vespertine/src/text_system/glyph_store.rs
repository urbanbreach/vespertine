//! Per-window state for run-level glyph painting (TXT-005).
//!
//! Holds the direct-mapped raster-bounds store, the recently used font bounding
//! boxes, the translation-stable origin quantization, the conservative glyph
//! reach used for culling, and the test-support counters that make the
//! per-run / per-line decisions observable.

use crate::{
    AtlasTile, Bounds, ContentMask, DevicePixels, FontId, Hsla, IsZero, Pixels, Point,
    RenderGlyphParams, Result, SUBPIXEL_VARIANTS_X, SUBPIXEL_VARIANTS_Y, ScaledPixels, Size,
};
use smallvec::SmallVec;
use std::hash::{Hash, Hasher};

#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
use std::cell::Cell;

/// Number of direct-mapped glyph slots kept per window.
pub(crate) const RASTER_STORE_SLOTS: usize = 4096;

/// Number of (font, size) bounding boxes kept per window.
pub(crate) const RECENT_FONT_BOUNDS: usize = 16;

/// Safety margin, in logical pixels, added to the conservative glyph reach.
pub(crate) const GLYPH_REACH_MARGIN: f32 = 2.0;

/// Counts of the decisions made while painting glyphs, for tests and performance diagnostics.
///
/// Read with `Window::glyph_paint_counters` and cleared with
/// `Window::reset_glyph_paint_counters`.
#[cfg(any(test, feature = "test-support", feature = "bench-support"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlyphPaintCounters {
    /// Rendering-policy calculations (subpixel eligibility plus color dilation).
    pub render_policy_calculations: u64,
    /// Content-mask snaps performed for glyph painting.
    pub mask_snaps: u64,
    /// Conservative glyph reach calculations.
    pub reach_calculations: u64,
    /// Sprite atlas accesses made for glyphs.
    pub atlas_accesses: u64,
    /// Raster-bounds queries forwarded to the text system.
    pub raster_bounds_queries: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum GlyphCounter {
    RenderPolicy,
    MaskSnap,
    Reach,
    AtlasAccess,
    RasterBoundsQuery,
}

#[derive(Default)]
pub(crate) struct GlyphCounters {
    #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
    cells: [Cell<u64>; 5],
}

impl GlyphCounters {
    #[inline]
    pub(crate) fn bump(&self, counter: GlyphCounter) {
        #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
        {
            let cell = &self.cells[counter as usize];
            cell.set(cell.get() + 1);
        }
        #[cfg(not(any(test, feature = "test-support", feature = "bench-support")))]
        let _ = counter;
    }

    #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
    pub(crate) fn snapshot(&self) -> GlyphPaintCounters {
        let get = |counter: GlyphCounter| self.cells[counter as usize].get();
        GlyphPaintCounters {
            render_policy_calculations: get(GlyphCounter::RenderPolicy),
            mask_snaps: get(GlyphCounter::MaskSnap),
            reach_calculations: get(GlyphCounter::Reach),
            atlas_accesses: get(GlyphCounter::AtlasAccess),
            raster_bounds_queries: get(GlyphCounter::RasterBoundsQuery),
        }
    }

    #[cfg(any(test, feature = "test-support", feature = "bench-support"))]
    pub(crate) fn reset(&self) {
        for cell in &self.cells {
            cell.set(0);
        }
    }
}

/// Splits one origin coordinate (in device pixels) into a whole-pixel origin and a subpixel variant.
///
/// `variants` is the number of subpixel variants on the axis. Halves round to the lower
/// step, so quantizing `x + n` equals quantizing `x` and adding `n` to the whole-pixel
/// origin, for negative `x` as well.
#[inline]
pub(crate) fn quantize_glyph_axis(value: f32, variants: u8) -> (f32, u8) {
    let variants = f32::from(variants);
    let whole = value.floor();
    let fraction = value - whole;
    let steps = (fraction * variants - 0.5).ceil().max(0.0);
    let carry = (steps / variants).floor();
    (whole + carry, (steps - carry * variants) as u8)
}

/// Whole-pixel origin of an emoji coordinate: the floor, plus one if the fraction exceeds one half.
#[inline]
pub(crate) fn quantize_emoji_axis(value: f32) -> f32 {
    let whole = value.floor();
    if value - whole <= 0.5 {
        whole
    } else {
        whole + 1.0
    }
}

/// Quantizes a monochrome or subpixel glyph origin (device pixels) into its whole-pixel
/// origin and subpixel variant (4 on x, 1 on y).
#[inline]
pub(crate) fn quantize_glyph_origin(
    origin: Point<ScaledPixels>,
) -> (Point<ScaledPixels>, Point<u8>) {
    let (x, variant_x) = quantize_glyph_axis(origin.x.0, SUBPIXEL_VARIANTS_X);
    let (y, variant_y) = quantize_glyph_axis(origin.y.0, SUBPIXEL_VARIANTS_Y);
    (
        Point::new(ScaledPixels(x), ScaledPixels(y)),
        Point::new(variant_x, variant_y),
    )
}

/// Quantizes an emoji origin (device pixels) into its whole-pixel origin.
#[inline]
pub(crate) fn quantize_emoji_origin(origin: Point<ScaledPixels>) -> Point<ScaledPixels> {
    Point::new(
        ScaledPixels(quantize_emoji_axis(origin.x.0)),
        ScaledPixels(quantize_emoji_axis(origin.y.0)),
    )
}

/// The area in which a glyph origin can reach a content mask.
///
/// A glyph with origin `x` and baseline `y` is painted if
/// `L - (bw + 2) < x < R + (bh + 2)` and `T - (bh + 2) < y < B + (bh + 2)`, where `bw` and
/// `bh` are the width and height of the font's bounding box and `L`, `T`, `R`, `B` the edges
/// of the content mask. The right margin uses the height on purpose: a glyph may extend to
/// the left of its origin by up to that height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlyphReach {
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

impl GlyphReach {
    /// A reach that no glyph is inside of.
    pub(crate) const NONE: GlyphReach = GlyphReach {
        min_x: f32::INFINITY,
        max_x: f32::NEG_INFINITY,
        min_y: f32::INFINITY,
        max_y: f32::NEG_INFINITY,
    };

    pub(crate) fn new(mask: Bounds<Pixels>, bounding_box: Size<Pixels>) -> Self {
        if mask.is_empty() {
            return Self::NONE;
        }
        let width = bounding_box.width.0 + GLYPH_REACH_MARGIN;
        let height = bounding_box.height.0 + GLYPH_REACH_MARGIN;
        Self {
            min_x: mask.left().0 - width,
            max_x: mask.right().0 + height,
            min_y: mask.top().0 - height,
            max_y: mask.bottom().0 + height,
        }
    }

    /// Returns whether a glyph with the given origin and baseline may reach the mask.
    #[inline]
    pub(crate) fn contains(&self, x: Pixels, baseline_y: Pixels) -> bool {
        x.0 > self.min_x
            && x.0 < self.max_x
            && baseline_y.0 > self.min_y
            && baseline_y.0 < self.max_y
    }
}

/// Decisions that hold for a run of consecutive glyphs sharing a font and a color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlyphRunStyle {
    pub font_id: FontId,
    pub color: Hsla,
    pub subpixel_rendering: bool,
    pub dilation: u8,
}

impl GlyphRunStyle {
    #[inline]
    pub(crate) fn matches(&self, font_id: FontId, color: Hsla) -> bool {
        self.font_id == font_id && self.color == color
    }
}

/// Decisions that hold for all glyphs painted from one line.
pub(crate) struct GlyphLinePaint {
    /// The content mask rounded outward to whole device pixels.
    pub snapped_mask: ContentMask<ScaledPixels>,
    /// The content mask in logical pixels, used for reach tests.
    pub mask: Bounds<Pixels>,
    pub scale_factor: f32,
    pub opacity: f32,
    reaches: SmallVec<[(u32, u32, GlyphReach); 2]>,
}

impl GlyphLinePaint {
    pub(crate) fn new(
        snapped_mask: ContentMask<ScaledPixels>,
        mask: Bounds<Pixels>,
        scale_factor: f32,
        opacity: f32,
    ) -> Self {
        Self {
            snapped_mask,
            mask,
            scale_factor,
            opacity,
            reaches: SmallVec::new(),
        }
    }

    /// Returns the reach for a bounding-box size, calling `calculated` when it is computed
    /// rather than reused from an earlier run of the line with a bit-identical size.
    pub(crate) fn reach(
        &mut self,
        bounding_box: Size<Pixels>,
        calculated: impl FnOnce(),
    ) -> GlyphReach {
        let width = bounding_box.width.0.to_bits();
        let height = bounding_box.height.0.to_bits();
        if let Some((_, _, reach)) = self
            .reaches
            .iter()
            .find(|(w, h, _)| *w == width && *h == height)
        {
            return *reach;
        }
        calculated();
        let reach = GlyphReach::new(self.mask, bounding_box);
        self.reaches.push((width, height, reach));
        reach
    }
}

/// Bounding boxes of the most recently used (font, size) pairs.
#[derive(Default)]
pub(crate) struct RecentFontBounds {
    entries: SmallVec<[(FontId, u32, Bounds<Pixels>); RECENT_FONT_BOUNDS]>,
}

impl RecentFontBounds {
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns the bounding box for a font and size, computing it with `compute` on a miss and
    /// dropping the least recently used pair when the list is full.
    pub(crate) fn get_or_insert_with(
        &mut self,
        font_id: FontId,
        font_size: Pixels,
        compute: impl FnOnce() -> Bounds<Pixels>,
    ) -> Bounds<Pixels> {
        let size_bits = font_size.0.to_bits();
        if let Some(ix) = self
            .entries
            .iter()
            .position(|(id, bits, _)| *id == font_id && *bits == size_bits)
        {
            if ix > 0 {
                self.entries[..=ix].rotate_right(1);
            }
            return self.entries[0].2;
        }
        let bounds = compute();
        if self.entries.len() == RECENT_FONT_BOUNDS {
            self.entries.pop();
        }
        self.entries.insert(0, (font_id, size_bits, bounds));
        bounds
    }
}

struct RasterSlot {
    params: RenderGlyphParams,
    bounds: Bounds<DevicePixels>,
    /// The sprite's pixel size. It only depends on the render request, so it is kept across frames.
    sprite_size: Option<Size<DevicePixels>>,
    /// The atlas tile and the frame in which it was looked up.
    tile: Option<(AtlasTile, u64)>,
}

/// A glyph sprite resolved by [`RasterStore::resolve`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphSprite {
    pub tile: AtlasTile,
    pub bounds: Bounds<ScaledPixels>,
}

/// A direct-mapped store of raster bounds, known sprite sizes and same-frame atlas tiles.
pub(crate) struct RasterStore {
    slots: Vec<Option<RasterSlot>>,
}

impl RasterStore {
    pub(crate) const fn new() -> Self {
        Self { slots: Vec::new() }
    }

    /// Hash of every field of the request, reduced to a slot index.
    pub(crate) fn slot_index(params: &RenderGlyphParams) -> usize {
        let mut hasher = SlotHasher(0);
        params.font_id.hash(&mut hasher);
        params.glyph_id.0.hash(&mut hasher);
        params.font_size.0.to_bits().hash(&mut hasher);
        params.subpixel_variant.x.hash(&mut hasher);
        params.subpixel_variant.y.hash(&mut hasher);
        params.scale_factor.to_bits().hash(&mut hasher);
        params.is_emoji.hash(&mut hasher);
        params.subpixel_rendering.hash(&mut hasher);
        params.dilation.hash(&mut hasher);
        // The multiplicative hash mixes into the high bits.
        (hasher.0 >> (64 - RASTER_STORE_SLOTS.trailing_zeros())) as usize
    }

    pub(crate) fn clear(&mut self) {
        self.slots.iter_mut().for_each(|slot| *slot = None);
    }

    /// Returns the number of occupied slots.
    #[cfg(test)]
    pub(crate) fn occupied(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// Resolves the sprite of a glyph request placed at a whole-pixel origin.
    ///
    /// Returns `None` if the glyph has no pixels, or if its sprite size is already known
    /// and the sprite lies fully outside `mask`; in the latter case the atlas is never
    /// touched. A miss asks `query_bounds` for the raster bounds; a stored tile is reused
    /// only when it was looked up in `frame`, otherwise `make_tile` provides it.
    pub(crate) fn resolve(
        &mut self,
        params: &RenderGlyphParams,
        integer_origin: Point<ScaledPixels>,
        mask: &ContentMask<ScaledPixels>,
        frame: u64,
        query_bounds: impl FnOnce(&RenderGlyphParams) -> Result<Bounds<DevicePixels>>,
        make_tile: impl FnOnce() -> Result<AtlasTile>,
    ) -> Result<Option<GlyphSprite>> {
        if self.slots.is_empty() {
            self.slots.resize_with(RASTER_STORE_SLOTS, || None);
        }
        let entry = &mut self.slots[Self::slot_index(params)];
        if !entry.as_ref().is_some_and(|slot| slot.params == *params) {
            let bounds = query_bounds(params)?;
            *entry = Some(RasterSlot {
                params: params.clone(),
                bounds,
                sprite_size: None,
                tile: None,
            });
        }
        let slot = entry.as_mut().expect("slot was just filled");
        if slot.bounds.is_zero() {
            return Ok(None);
        }

        let origin = integer_origin + slot.bounds.origin.map(Into::into);
        if let Some(size) = slot.sprite_size {
            let known = Bounds {
                origin,
                size: size.map(Into::into),
            };
            if known.intersect(&mask.bounds).is_empty() {
                return Ok(None);
            }
        }

        let tile = match slot.tile {
            Some((tile, tile_frame)) if tile_frame == frame => tile,
            _ => {
                let tile = make_tile()?;
                slot.sprite_size = Some(tile.bounds.size);
                slot.tile = Some((tile, frame));
                tile
            }
        };
        Ok(Some(GlyphSprite {
            tile,
            bounds: Bounds {
                origin,
                size: tile.bounds.size.map(Into::into),
            },
        }))
    }
}

/// Fibonacci-style multiplicative hasher; the slot index is taken from the high bits.
struct SlotHasher(u64);

impl SlotHasher {
    const SEED: u64 = 0x9E37_79B9_7F4A_7C15;

    #[inline]
    fn mix(&mut self, value: u64) {
        self.0 = (self.0.rotate_left(5) ^ value).wrapping_mul(Self::SEED);
    }
}

impl Hasher for SlotHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.mix(u64::from(byte));
        }
    }

    fn write_u8(&mut self, value: u8) {
        self.mix(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.mix(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.mix(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.mix(value as u64);
    }
}

/// Everything a window keeps for glyph painting.
pub(crate) struct GlyphStore {
    pub raster: RasterStore,
    pub fonts: RecentFontBounds,
    generation: usize,
    frame: u64,
    pub counters: GlyphCounters,
}

impl GlyphStore {
    pub(crate) fn new() -> Self {
        Self {
            raster: RasterStore::new(),
            fonts: RecentFontBounds::default(),
            generation: 0,
            frame: 0,
            counters: GlyphCounters::default(),
        }
    }

    /// Empties the store when the font generation changed, because font identities may be reinterpreted.
    pub(crate) fn sync_generation(&mut self, generation: usize) {
        if self.generation != generation {
            self.generation = generation;
            self.raster.clear();
            self.fonts.clear();
        }
    }

    /// Starts a new frame; tiles looked up in earlier frames are no longer reused.
    pub(crate) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(1);
    }

    pub(crate) fn frame(&self) -> u64 {
        self.frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AtlasTextureId, AtlasTextureKind, GlyphId, TileId, bounds, point, px, size};
    use std::cell::Cell;

    fn quantize_x(value: f32) -> (f32, u8) {
        quantize_glyph_axis(value, SUBPIXEL_VARIANTS_X)
    }

    fn quantize_y(value: f32) -> (f32, u8) {
        quantize_glyph_axis(value, SUBPIXEL_VARIANTS_Y)
    }

    /// The origin quantization of upstream GPUI: round half toward zero, truncate.
    fn base_quantize_x(value: f32) -> (f32, u8) {
        let quantized = crate::util::round_half_toward_zero(value * 4.0) / 4.0;
        (quantized.trunc(), (quantized.fract() * 4.0) as u8)
    }

    #[test]
    fn origin_quantization_table() {
        // Ties go to the lower step, also below zero.
        assert_eq!(quantize_x(-0.125), (-1.0, 3));
        assert_eq!(quantize_x(0.125), (0.0, 0));
        assert_eq!(quantize_x(-0.3), (-1.0, 3));
        assert_eq!(quantize_x(0.375), (0.0, 1));
        assert_eq!(quantize_x(0.0), (0.0, 0));
        assert_eq!(quantize_x(-1.0), (-1.0, 0));
        assert_eq!(quantize_x(0.9), (1.0, 0));
        assert_eq!(quantize_x(-0.9), (-1.0, 0));
        assert_eq!(quantize_x(0.25), (0.0, 1));
        assert_eq!(quantize_x(-0.25), (-1.0, 3));
        assert_eq!(quantize_x(-0.875), (-1.0, 0));
        assert_eq!(quantize_x(-0.625), (-1.0, 1));
        // One variant on y.
        assert_eq!(quantize_y(-0.5), (-1.0, 0));
        assert_eq!(quantize_y(0.5), (0.0, 0));
        assert_eq!(quantize_y(0.5001), (1.0, 0));
        assert_eq!(quantize_y(-0.4999), (0.0, 0));
        assert_eq!(quantize_y(-1.5), (-2.0, 0));
        // Emoji origins.
        assert_eq!(quantize_emoji_axis(-0.5), -1.0);
        assert_eq!(quantize_emoji_axis(0.5), 0.0);
        assert_eq!(quantize_emoji_axis(0.5001), 1.0);
        assert_eq!(quantize_emoji_axis(-0.4999), 0.0);
        assert_eq!(quantize_emoji_axis(-1.5), -2.0);
        assert_eq!(quantize_emoji_axis(2.0), 2.0);

        let (origin, variant) =
            quantize_glyph_origin(point(ScaledPixels(-0.125), ScaledPixels(-0.5)));
        assert_eq!(origin, point(ScaledPixels(-1.0), ScaledPixels(-1.0)));
        assert_eq!(variant, point(3, 0));
        let (origin, variant) =
            quantize_glyph_origin(point(ScaledPixels(0.125), ScaledPixels(0.5)));
        assert_eq!(origin, point(ScaledPixels(0.0), ScaledPixels(0.0)));
        assert_eq!(variant, point(0, 0));
        assert_eq!(
            quantize_emoji_origin(point(ScaledPixels(-0.5), ScaledPixels(0.5))),
            point(ScaledPixels(-1.0), ScaledPixels(0.0))
        );
    }

    /// Deterministic pseudo-random sequence (xorshift64).
    fn sequence(seed: u64, count: usize) -> impl Iterator<Item = u64> {
        let mut state = seed;
        std::iter::repeat_with(move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        })
        .take(count)
    }

    #[test]
    fn origin_quantization_is_translation_stable() {
        // Values are multiples of 1/256 so that `x + n` is exact in f32.
        let offsets = [0i32, 1, -1, 2, 3, -3, 7, -17, 64, -255, 1000, -4097];
        for raw in sequence(0x1234_5678_9abc_def1, 20_000) {
            let x = ((raw % (1 << 20)) as i64 - (1 << 19)) as f32 / 256.0;
            let (whole, variant) = quantize_x(x);
            let (whole_y, _) = quantize_y(x);
            let emoji = quantize_emoji_axis(x);
            for n in offsets {
                let shifted = x + n as f32;
                assert_eq!(
                    quantize_x(shifted),
                    (whole + n as f32, variant),
                    "x={x} n={n}"
                );
                assert_eq!(quantize_y(shifted).0, whole_y + n as f32, "y={x} n={n}");
                assert_eq!(
                    quantize_emoji_axis(shifted),
                    emoji + n as f32,
                    "emoji={x} n={n}"
                );
            }
            assert!(variant < 4);
        }
        // Integers and quarter pixels over a dense grid, both signs.
        for k in -4096i32..=4096 {
            let x = k as f32 / 4.0;
            let (whole, variant) = quantize_x(x);
            assert_eq!(variant, k.rem_euclid(4) as u8, "x={x}");
            assert_eq!(whole, x.floor(), "x={x}");
            for n in offsets {
                assert_eq!(quantize_x(x + n as f32), (whole + n as f32, variant));
            }
        }
    }

    #[test]
    fn origin_quantization_matches_base_for_non_negative_origins() {
        for raw in sequence(0xdead_beef_cafe_f00d, 20_000) {
            let x = (raw % (1 << 24)) as f32 / 1024.0;
            assert_eq!(quantize_x(x), base_quantize_x(x), "x={x}");
            let y = crate::util::round_half_toward_zero(x);
            assert_eq!(quantize_y(x).0, y, "y={x}");
            assert_eq!(quantize_emoji_axis(x), y, "emoji={x}");
        }
        for bits in sequence(0x0f0f_1234_aaaa_5555, 20_000).map(|raw| (raw >> 33) as u32) {
            let x = f32::from_bits(bits);
            // Beyond 2^21 the base's `abs - 0.5` is no longer exact in f32 and the base itself
            // mis-rounds exact quarter-pixel ties.
            if x.is_finite() && (0.0..(1u32 << 21) as f32).contains(&x) {
                assert_eq!(quantize_x(x), base_quantize_x(x), "x={x}");
            }
        }
    }

    #[test]
    fn reach_follows_baseline_and_margin() {
        let mask = bounds(point(px(10.), px(20.)), size(px(100.), px(40.)));
        let bounding_box = size(px(30.), px(50.));
        let reach = GlyphReach::new(mask, bounding_box);
        // L - (bw + 2) < x < R + (bh + 2); T - (bh + 2) < y < B + (bh + 2)
        assert!(reach.contains(px(-21.9), px(40.)));
        assert!(!reach.contains(px(-22.), px(40.)));
        assert!(reach.contains(px(161.9), px(40.)));
        assert!(!reach.contains(px(162.), px(40.)));
        assert!(reach.contains(px(50.), px(-31.9)));
        assert!(!reach.contains(px(50.), px(-32.)));
        assert!(reach.contains(px(50.), px(111.9)));
        assert!(!reach.contains(px(50.), px(112.)));
        // Empty masks reject every glyph.
        for empty in [
            bounds(point(px(0.), px(0.)), size(px(0.), px(10.))),
            bounds(point(px(0.), px(0.)), size(px(10.), px(0.))),
            bounds(point(px(0.), px(0.)), size(px(-10.), px(10.))),
        ] {
            let reach = GlyphReach::new(empty, bounding_box);
            assert!(!reach.contains(px(0.), px(0.)));
            assert!(!reach.contains(px(5.), px(5.)));
        }
    }

    #[test]
    fn line_reach_is_computed_once_per_distinct_bounding_box_size() {
        let mask = bounds(point(px(0.), px(0.)), size(px(100.), px(100.)));
        let mut line = GlyphLinePaint::new(
            ContentMask {
                bounds: Bounds::default(),
            },
            mask,
            1.0,
            1.0,
        );
        let calculations = Cell::new(0);
        let a = size(px(10.), px(12.));
        let b = size(px(11.), px(12.));
        for bounding_box in [a, a, b, a, b, a] {
            line.reach(bounding_box, || calculations.set(calculations.get() + 1));
        }
        assert_eq!(calculations.get(), 2);
    }

    #[test]
    fn recent_font_bounds_keep_sixteen_most_recent() {
        let mut recent = RecentFontBounds::default();
        let computed = Cell::new(0);
        let mut lookup = |recent: &mut RecentFontBounds, font: usize, font_size: f32| {
            recent.get_or_insert_with(FontId(font), px(font_size), || {
                computed.set(computed.get() + 1);
                bounds(
                    point(px(font as f32), px(0.)),
                    size(px(font_size), px(font_size)),
                )
            })
        };
        for font in 0..RECENT_FONT_BOUNDS {
            lookup(&mut recent, font, 14.);
        }
        assert_eq!(computed.get(), 16);
        assert_eq!(recent.len(), 16);
        // Hits do not compute; font 0 becomes the most recent.
        assert_eq!(lookup(&mut recent, 0, 14.).origin.x, px(0.));
        assert_eq!(computed.get(), 16);
        // The same font at another size is a distinct pair and drops the oldest (font 1).
        lookup(&mut recent, 0, 15.);
        assert_eq!(computed.get(), 17);
        assert_eq!(recent.len(), 16);
        lookup(&mut recent, 0, 14.);
        assert_eq!(computed.get(), 17);
        lookup(&mut recent, 1, 14.);
        assert_eq!(computed.get(), 18);
        lookup(&mut recent, 2, 14.);
        assert_eq!(
            computed.get(),
            19,
            "font 2 was dropped when font 1 re-entered"
        );
    }

    fn request(glyph: u32, variant_x: u8) -> RenderGlyphParams {
        RenderGlyphParams {
            font_id: FontId(3),
            glyph_id: GlyphId(glyph),
            font_size: px(14.),
            subpixel_variant: point(variant_x, 0),
            scale_factor: 2.0,
            is_emoji: false,
            subpixel_rendering: false,
            dilation: 0,
        }
    }

    fn tile(id: u32, width: i32, height: i32) -> AtlasTile {
        AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Monochrome,
            },
            tile_id: TileId(id),
            padding: 0,
            bounds: bounds(
                point(DevicePixels(0), DevicePixels(0)),
                size(DevicePixels(width), DevicePixels(height)),
            ),
        }
    }

    fn visible_mask() -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: bounds(
                point(ScaledPixels(0.), ScaledPixels(0.)),
                size(ScaledPixels(100.), ScaledPixels(100.)),
            ),
        }
    }

    /// A fake rasterizer: bounds identify the request, and every call is counted.
    struct Fake {
        queries: Cell<u32>,
        atlas_accesses: Cell<u32>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                queries: Cell::new(0),
                atlas_accesses: Cell::new(0),
            }
        }

        fn resolve(
            &self,
            store: &mut RasterStore,
            params: &RenderGlyphParams,
            origin: Point<ScaledPixels>,
            mask: &ContentMask<ScaledPixels>,
            frame: u64,
        ) -> Option<GlyphSprite> {
            store
                .resolve(
                    params,
                    origin,
                    mask,
                    frame,
                    |params| {
                        self.queries.set(self.queries.get() + 1);
                        // Encode the request in the bounds: width from the glyph, height from the variant.
                        Ok(bounds(
                            point(DevicePixels(1), DevicePixels(-2)),
                            size(
                                DevicePixels(params.glyph_id.0 as i32),
                                DevicePixels(10 + i32::from(params.subpixel_variant.x)),
                            ),
                        ))
                    },
                    || {
                        self.atlas_accesses.set(self.atlas_accesses.get() + 1);
                        Ok(tile(
                            params.glyph_id.0 * 10 + u32::from(params.subpixel_variant.x),
                            params.glyph_id.0 as i32,
                            10 + i32::from(params.subpixel_variant.x),
                        ))
                    },
                )
                .unwrap()
        }
    }

    fn origin(x: f32, y: f32) -> Point<ScaledPixels> {
        point(ScaledPixels(x), ScaledPixels(y))
    }

    #[test]
    fn store_hit_reuses_bounds_and_same_frame_tile() {
        let fake = Fake::new();
        let mut store = RasterStore::new();
        let params = request(5, 1);
        let first = fake
            .resolve(&mut store, &params, origin(10., 10.), &visible_mask(), 1)
            .unwrap();
        assert_eq!(first.bounds.origin, origin(11., 8.));
        assert_eq!(first.bounds.size, size(ScaledPixels(5.), ScaledPixels(11.)));
        let second = fake
            .resolve(&mut store, &params, origin(30., 10.), &visible_mask(), 1)
            .unwrap();
        assert_eq!(second.tile, first.tile);
        assert_eq!(second.bounds.origin, origin(31., 8.));
        assert_eq!((fake.queries.get(), fake.atlas_accesses.get()), (1, 1));
        assert_eq!(store.occupied(), 1);
    }

    #[test]
    fn colliding_requests_never_return_each_others_bounds() {
        // Find two distinct requests that map to the same slot.
        let first = request(1, 0);
        let slot = RasterStore::slot_index(&first);
        let second = (2..200_000)
            .map(|glyph| request(glyph, 0))
            .find(|candidate| RasterStore::slot_index(candidate) == slot)
            .expect("a colliding request exists");
        assert_ne!(first, second);

        let fake = Fake::new();
        let mut store = RasterStore::new();
        let a = fake
            .resolve(&mut store, &first, origin(0., 0.), &visible_mask(), 1)
            .unwrap();
        let b = fake
            .resolve(&mut store, &second, origin(0., 0.), &visible_mask(), 1)
            .unwrap();
        assert_ne!(a.bounds.size, b.bounds.size);
        assert_eq!(b.bounds.size.width, ScaledPixels(second.glyph_id.0 as f32));
        // The second request replaced the first: asking for the first again re-queries.
        let again = fake
            .resolve(&mut store, &first, origin(0., 0.), &visible_mask(), 1)
            .unwrap();
        assert_eq!(again.bounds.size, a.bounds.size);
        assert_eq!(fake.queries.get(), 3);
        assert_eq!(store.occupied(), 1);

        // Requests differing in any single field are distinct entries.
        let base = request(7, 2);
        let variants = [
            RenderGlyphParams {
                font_id: FontId(4),
                ..base.clone()
            },
            RenderGlyphParams {
                font_size: px(15.),
                ..base.clone()
            },
            RenderGlyphParams {
                scale_factor: 1.5,
                ..base.clone()
            },
            RenderGlyphParams {
                subpixel_variant: point(2, 1),
                ..base.clone()
            },
            RenderGlyphParams {
                is_emoji: true,
                ..base.clone()
            },
            RenderGlyphParams {
                subpixel_rendering: true,
                ..base.clone()
            },
            RenderGlyphParams {
                dilation: 1,
                ..base
            },
        ];
        for variant in variants {
            let fake = Fake::new();
            let mut store = RasterStore::new();
            fake.resolve(&mut store, &base, origin(0., 0.), &visible_mask(), 1);
            fake.resolve(&mut store, &variant, origin(0., 0.), &visible_mask(), 1);
            assert_eq!(fake.queries.get(), 2, "{variant:?}");
        }
    }

    #[test]
    fn font_generation_change_empties_the_store() {
        let fake = Fake::new();
        let mut glyphs = GlyphStore::new();
        glyphs.sync_generation(0);
        let params = request(4, 0);
        fake.resolve(
            &mut glyphs.raster,
            &params,
            origin(0., 0.),
            &visible_mask(),
            1,
        );
        glyphs
            .fonts
            .get_or_insert_with(FontId(1), px(14.), Bounds::default);
        assert_eq!(glyphs.raster.occupied(), 1);
        glyphs.sync_generation(0);
        assert_eq!(
            glyphs.raster.occupied(),
            1,
            "unchanged generation keeps entries"
        );
        glyphs.sync_generation(1);
        assert_eq!(glyphs.raster.occupied(), 0);
        assert_eq!(glyphs.fonts.len(), 0);
        fake.resolve(
            &mut glyphs.raster,
            &params,
            origin(0., 0.),
            &visible_mask(),
            1,
        );
        assert_eq!(fake.queries.get(), 2);
        assert_eq!(fake.atlas_accesses.get(), 2);
    }

    #[test]
    fn tile_is_never_reused_in_a_later_frame() {
        let fake = Fake::new();
        let mut store = RasterStore::new();
        let params = request(6, 3);
        for frame in [1, 1, 2, 2, 2, 5, 5] {
            fake.resolve(&mut store, &params, origin(0., 0.), &visible_mask(), frame)
                .unwrap();
        }
        // One atlas access per distinct frame, one raster-bounds query overall.
        assert_eq!(fake.atlas_accesses.get(), 3);
        assert_eq!(fake.queries.get(), 1);
    }

    #[test]
    fn known_sprite_sizes_cull_before_atlas_access() {
        let fake = Fake::new();
        let mut store = RasterStore::new();
        let params = request(8, 0); // sprite 8 x 10 at origin + (1, -2)
        let mask = visible_mask();

        // Unknown size: the glyph goes to the atlas even if it is outside the mask.
        assert!(
            fake.resolve(&mut store, &params, origin(500., 500.), &mask, 1)
                .is_some()
        );
        assert_eq!(fake.atlas_accesses.get(), 1);

        // Known size, in a later frame: invisible glyphs are culled without touching the atlas.
        for outside in [
            origin(500., 50.),
            origin(50., 500.),
            origin(-20., 50.),
            origin(50., -30.),
            origin(-9., 50.),
        ] {
            assert!(
                fake.resolve(&mut store, &params, outside, &mask, 2)
                    .is_none(),
                "{outside:?}"
            );
        }
        assert_eq!(fake.atlas_accesses.get(), 1);
        assert_eq!(fake.queries.get(), 1);

        // Visible and edge-touching glyphs still reach the atlas.
        assert!(
            fake.resolve(&mut store, &params, origin(50., 50.), &mask, 2)
                .is_some()
        );
        assert_eq!(fake.atlas_accesses.get(), 2);
        assert!(
            fake.resolve(&mut store, &params, origin(-7., 50.), &mask, 2)
                .is_some(),
            "sprite spans x in [-6, 2)"
        );

        // Empty glyphs never reach the atlas.
        let empty = request(0, 0);
        let before = fake.atlas_accesses.get();
        assert!(
            fake.resolve(&mut store, &empty, origin(10., 10.), &mask, 2)
                .is_none()
        );
        assert_eq!(fake.atlas_accesses.get(), before);
    }
}

/// Tests that drive glyph painting through a real window with a text system that has glyph pixels.
#[cfg(test)]
mod window_tests {
    use super::*;
    use crate::AppContext as _;
    use crate::{
        App, Context, FontMetrics, FontRun, GlyphId, IntoElement, LineLayout, NoopTextSystem,
        PlatformTextSystem, Render, ShapedLine, SharedString, Styled, TestAppContext,
        TestDispatcher, TextAlign, TextRenderingMode, TextRun, Window, black, bounds, canvas, font,
        hsla, point, px, size,
    };
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    /// A text system whose glyphs have pixels: monochrome glyphs are `10 + variant` by 12
    /// device pixels, so the sprite size reveals the quantized variant; emoji are 14 x 14.
    struct RasterTextSystem {
        inner: NoopTextSystem,
        bounds_queries: AtomicUsize,
    }

    impl RasterTextSystem {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                inner: NoopTextSystem::new(),
                bounds_queries: AtomicUsize::new(0),
            })
        }
    }

    impl PlatformTextSystem for RasterTextSystem {
        fn add_fonts(&self, fonts: Vec<std::borrow::Cow<'static, [u8]>>) -> Result<()> {
            self.inner.add_fonts(fonts)
        }

        fn all_font_names(&self) -> Vec<String> {
            self.inner.all_font_names()
        }

        fn font_id(&self, descriptor: &crate::Font) -> Result<FontId> {
            self.inner.font_id(descriptor)
        }

        fn font_metrics(&self, font_id: FontId) -> FontMetrics {
            self.inner.font_metrics(font_id)
        }

        fn typographic_bounds(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Bounds<f32>> {
            self.inner.typographic_bounds(font_id, glyph_id)
        }

        fn advance(&self, font_id: FontId, glyph_id: GlyphId) -> Result<Size<f32>> {
            self.inner.advance(font_id, glyph_id)
        }

        fn glyph_for_char(&self, font_id: FontId, ch: char) -> Option<GlyphId> {
            self.inner.glyph_for_char(font_id, ch)
        }

        fn glyph_raster_bounds(&self, params: &RenderGlyphParams) -> Result<Bounds<DevicePixels>> {
            self.bounds_queries.fetch_add(1, Ordering::SeqCst);
            Ok(if params.is_emoji {
                bounds(
                    point(DevicePixels(0), DevicePixels(-12)),
                    size(DevicePixels(14), DevicePixels(14)),
                )
            } else {
                bounds(
                    point(DevicePixels(-1), DevicePixels(-10)),
                    size(
                        DevicePixels(10 + i32::from(params.subpixel_variant.x)),
                        DevicePixels(12),
                    ),
                )
            })
        }

        fn rasterize_glyph(
            &self,
            _params: &RenderGlyphParams,
            raster_bounds: Bounds<DevicePixels>,
        ) -> Result<(Size<DevicePixels>, Vec<u8>)> {
            Ok((raster_bounds.size, Vec::new()))
        }

        fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
            self.inner.layout_line(text, font_size, runs)
        }

        fn recommended_rendering_mode(
            &self,
            font_id: FontId,
            font_size: Pixels,
        ) -> TextRenderingMode {
            self.inner.recommended_rendering_mode(font_id, font_size)
        }
    }

    struct PaintView(Rc<dyn Fn(&mut Window, &mut App)>);

    impl Render for PaintView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let paint = self.0.clone();
            canvas(|_, _, _| {}, move |_, _, window, cx| paint(window, cx)).size_full()
        }
    }

    fn test_context() -> (TestAppContext, Arc<RasterTextSystem>) {
        let text_system = RasterTextSystem::new();
        let cx = TestAppContext::build_with_text_system(
            TestDispatcher::new(0),
            None,
            text_system.clone(),
        );
        (cx, text_system)
    }

    /// Paints at the given scale factor; assertions inside `paint` fail the test.
    fn run_paint(scale: f32, paint: impl Fn(&mut Window, &mut App) + 'static) {
        let (mut cx, _) = test_context();
        let window = cx.add_window(move |_, _| PaintView(Rc::new(paint)));
        cx.simulate_window_scale_factor_change(window.into(), scale);
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    fn shape(
        window: &Window,
        text: &str,
        font_size: f32,
        runs: &[(usize, Hsla, Option<Hsla>)],
    ) -> ShapedLine {
        let runs: Vec<TextRun> = runs
            .iter()
            .map(|&(len, color, background_color)| TextRun {
                len,
                font: font(".ZedMono"),
                color,
                background_color,
                underline: None,
                strikethrough: None,
            })
            .collect();
        window
            .text_system()
            .shape_line(SharedString::new(text), px(font_size), &runs, None)
    }

    fn paint(
        line: &ShapedLine,
        origin: Point<Pixels>,
        line_height: f32,
        window: &mut Window,
        cx: &mut App,
    ) {
        line.paint(origin, px(line_height), TextAlign::Left, None, window, cx)
            .unwrap();
    }

    #[test]
    fn warmed_run_makes_one_decision_per_run_line_and_bounding_box_size() {
        run_paint(2.0, |window, cx| {
            let red = hsla(0., 1., 0.5, 1.);
            let blue = hsla(0.6, 1., 0.5, 1.);
            let origin = point(px(40.), px(40.));

            // Three runs of consecutive glyphs sharing a color: red, blue, red.
            let line = shape(
                window,
                "abcdefgh",
                16.,
                &[(2, red, None), (3, blue, None), (3, red, None)],
            );
            paint(&line, origin, 20., window, cx);
            window.reset_glyph_paint_counters();
            let sprites = window.next_frame.scene.monochrome_sprites.len();
            paint(&line, origin, 20., window, cx);
            assert_eq!(
                window.next_frame.scene.monochrome_sprites.len() - sprites,
                8
            );
            assert_eq!(
                window.glyph_paint_counters(),
                GlyphPaintCounters {
                    render_policy_calculations: 3,
                    mask_snaps: 1,
                    reach_calculations: 1,
                    // Warmed: raster bounds and same-frame tiles come from the store.
                    atlas_accesses: 0,
                    raster_bounds_queries: 0,
                }
            );

            // One color: one policy calculation for eight glyphs; two lines: two mask snaps.
            let plain = shape(window, "abcdefgh", 16., &[(8, red, None)]);
            paint(&plain, origin, 20., window, cx);
            window.reset_glyph_paint_counters();
            paint(&plain, origin, 20., window, cx);
            paint(&plain, origin, 20., window, cx);
            let counters = window.glyph_paint_counters();
            assert_eq!(counters.render_policy_calculations, 2);
            assert_eq!(counters.mask_snaps, 2);
            assert_eq!(counters.reach_calculations, 2);
            assert_eq!(counters.atlas_accesses, 0);
            assert_eq!(counters.raster_bounds_queries, 0);
        });
    }

    #[test]
    fn tiles_are_looked_up_again_in_the_next_frame_but_bounds_are_kept() {
        let (mut cx, text_system) = test_context();
        let seen = Rc::new(RefCell::new(Vec::new()));
        let recorded = seen.clone();
        let window = cx.add_window(move |_, _| {
            PaintView(Rc::new(move |window, cx| {
                let line = shape(window, "abcdefgh", 16., &[(8, black(), None)]);
                window.reset_glyph_paint_counters();
                paint(&line, point(px(40.), px(40.)), 20., window, cx);
                recorded.borrow_mut().push(window.glyph_paint_counters());
            }))
        });
        for _ in 0..2 {
            cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
                .unwrap();
        }
        let seen = seen.borrow();
        let [.., previous, last] = seen.as_slice() else {
            panic!("expected at least two frames");
        };
        assert!(previous.atlas_accesses > 0);
        assert_eq!(last.atlas_accesses, previous.atlas_accesses);
        assert_eq!(last.raster_bounds_queries, 0);
        assert_eq!(last.render_policy_calculations, 1);
        assert!(text_system.bounds_queries.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn translated_glyphs_match_direct_glyphs_exactly() {
        for scale in [1.0, 2.0] {
            run_paint(scale, |window, _| {
                let scale = window.scale_factor();
                let wide = ContentMask {
                    bounds: bounds(
                        point(ScaledPixels(-1e6), ScaledPixels(-1e6)),
                        size(ScaledPixels(2e6), ScaledPixels(2e6)),
                    ),
                };
                let mut line = window.begin_glyph_line();
                line.snapped_mask = wide;
                line.mask = wide.bounds.map(|c| px(c.0 / scale));
                let style = window.glyph_run_style(FontId(0), px(16.), black());
                let at = |device: Point<f32>| point(px(device.x / scale), px(device.y / scale));

                // Device origins (multiples of 1/8 so that translation is exact).
                let origins = [
                    (-0.125, -0.5),
                    (0.125, 0.5),
                    (-0.3, 0.25),
                    (-0.625, -1.5),
                    (0.0, 0.0),
                    (3.375, -7.625),
                    (-12.875, 5.5),
                    (0.5, -0.5),
                ];
                let translations = [
                    (0., 0.),
                    (1., -1.),
                    (-1., 1.),
                    (2., 3.),
                    (-5., 7.),
                    (13., -11.),
                    (-64., 64.),
                    (1001., -1001.),
                ];
                for (x, y) in origins {
                    let paint_glyph = |window: &mut Window, device: Point<f32>| {
                        let before = window.next_frame.scene.monochrome_sprites.len();
                        window
                            .paint_line_glyph(&line, &style, at(device), GlyphId(1), px(16.))
                            .unwrap();
                        let sprites = &window.next_frame.scene.monochrome_sprites;
                        assert_eq!(sprites.len(), before + 1);
                        sprites[before]
                    };
                    let paint_emoji = |window: &mut Window, device: Point<f32>| {
                        let before = window.next_frame.scene.polychrome_sprites.len();
                        window
                            .paint_line_emoji(&line, FontId(0), at(device), GlyphId(2), px(16.))
                            .unwrap();
                        let sprites = &window.next_frame.scene.polychrome_sprites;
                        assert_eq!(sprites.len(), before + 1);
                        sprites[before]
                    };
                    let base = paint_glyph(window, point(x, y));
                    let base_emoji = paint_emoji(window, point(x, y));
                    for (dx, dy) in translations {
                        let moved = paint_glyph(window, point(x + dx, y + dy));
                        assert_eq!(moved.tile, base.tile, "origin ({x}, {y}) by ({dx}, {dy})");
                        assert_eq!(
                            moved.bounds.size, base.bounds.size,
                            "variant at ({x}, {y}) by ({dx}, {dy})"
                        );
                        assert_eq!(
                            moved.bounds.origin,
                            base.bounds.origin + point(ScaledPixels(dx), ScaledPixels(dy)),
                            "origin ({x}, {y}) by ({dx}, {dy})"
                        );
                        assert_eq!(moved.content_mask, base.content_mask);
                        assert_eq!(moved.color, base.color);

                        let moved_emoji = paint_emoji(window, point(x + dx, y + dy));
                        assert_eq!(moved_emoji.tile, base_emoji.tile);
                        assert_eq!(
                            moved_emoji.bounds.origin,
                            base_emoji.bounds.origin + point(ScaledPixels(dx), ScaledPixels(dy)),
                            "emoji ({x}, {y}) by ({dx}, {dy})"
                        );
                    }
                }

                // The ties of the specification, observed on the sprites
                // (monochrome glyphs: origin (-1, -10), width 10 + variant).
                let sprite_at = |window: &mut Window, x: f32, y: f32| {
                    let before = window.next_frame.scene.monochrome_sprites.len();
                    window
                        .paint_line_glyph(&line, &style, at(point(x, y)), GlyphId(1), px(16.))
                        .unwrap();
                    window.next_frame.scene.monochrome_sprites[before]
                };
                let sprite = sprite_at(window, -0.125, -0.5);
                assert_eq!(
                    sprite.bounds.origin,
                    point(ScaledPixels(-2.), ScaledPixels(-11.))
                );
                assert_eq!(sprite.bounds.size.width, ScaledPixels(13.), "variant 3");
                let sprite = sprite_at(window, 0.125, 0.5);
                assert_eq!(
                    sprite.bounds.origin,
                    point(ScaledPixels(-1.), ScaledPixels(-10.))
                );
                assert_eq!(sprite.bounds.size.width, ScaledPixels(10.), "variant 0");
                let sprite = sprite_at(window, -0.3, 0.0);
                assert_eq!(sprite.bounds.origin.x, ScaledPixels(-2.));
                assert_eq!(sprite.bounds.size.width, ScaledPixels(13.), "variant 3");

                // Emoji (origin (0, -12)).
                let emoji_at = |window: &mut Window, x: f32| {
                    let before = window.next_frame.scene.polychrome_sprites.len();
                    window
                        .paint_line_emoji(&line, FontId(0), at(point(x, x)), GlyphId(2), px(16.))
                        .unwrap();
                    window.next_frame.scene.polychrome_sprites[before]
                };
                assert_eq!(
                    emoji_at(window, -0.5).bounds.origin,
                    point(ScaledPixels(-1.), ScaledPixels(-13.))
                );
                assert_eq!(
                    emoji_at(window, 0.5).bounds.origin,
                    point(ScaledPixels(0.), ScaledPixels(-12.))
                );
            });
        }
    }

    /// The baseline of the first line of a shaped line placed at `origin`.
    fn baseline(line: &ShapedLine, origin: Point<Pixels>, line_height: f32) -> Pixels {
        let padding_top = (px(line_height) - line.ascent - line.descent) / 2.;
        origin.y + padding_top + line.ascent
    }

    #[test]
    fn line_box_above_the_clip_with_baseline_inside_still_paints() {
        // Clip E of the fidelity scene: clip at (20, 290), 260 x 40; a 24 px line with a
        // 120 px line box placed at (12, -60) inside the clip.
        run_paint(2.0, |window, cx| {
            let line = shape(window, "Agjp 012345", 24., &[(11, black(), None)]);
            let clip = bounds(point(px(20.), px(290.)), size(px(260.), px(40.)));
            let origin = point(px(32.), px(230.));
            let bounding_box = window.text_system().bounding_box(FontId(0), px(24.));
            assert!(
                origin.y + bounding_box.size.height < clip.top(),
                "the box of the font's bounding size at the line top misses the clip"
            );
            let baseline = baseline(&line, origin, 120.);
            assert!(baseline > clip.top() && baseline < clip.bottom());

            let before = window.next_frame.scene.monochrome_sprites.len();
            window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                paint(&line, origin, 120., window, cx)
            });
            let sprites = &window.next_frame.scene.monochrome_sprites[before..];
            assert_eq!(sprites.len(), 11, "every glyph of the line is painted");
            let snapped = bounds(
                point(ScaledPixels(40.), ScaledPixels(580.)),
                size(ScaledPixels(520.), ScaledPixels(80.)),
            );
            for sprite in sprites {
                assert_eq!(sprite.content_mask.bounds, snapped);
                assert!(!sprite.bounds.intersect(&snapped).is_empty());
            }
        });
    }

    #[test]
    fn glyphs_that_cannot_reach_the_mask_are_culled_before_any_lookup() {
        run_paint(2.0, |window, cx| {
            let line = shape(window, "Agjp 012345", 24., &[(11, black(), None)]);
            let clip = bounds(point(px(20.), px(290.)), size(px(260.), px(40.)));
            let bounding_box = window.text_system().bounding_box(FontId(0), px(24.));
            let margin = bounding_box.size.height + px(2.);
            let line_height = 20.;
            let paint_clipped = |window: &mut Window, cx: &mut App, origin: Point<Pixels>| {
                window.reset_glyph_paint_counters();
                let before = window.next_frame.scene.monochrome_sprites.len();
                window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                    paint(&line, origin, line_height, window, cx)
                });
                (
                    window.next_frame.scene.monochrome_sprites.len() - before,
                    window.glyph_paint_counters(),
                )
            };
            let to_baseline = baseline(&line, point(px(0.), px(0.)), line_height);

            // Baseline just below the lower reach edge: culled, no store or atlas access.
            let below = point(px(40.), clip.bottom() + margin - to_baseline + px(0.5));
            let (sprites, counters) = paint_clipped(window, cx, below);
            assert_eq!(sprites, 0);
            assert_eq!(counters.atlas_accesses, 0);
            assert_eq!(counters.raster_bounds_queries, 0);
            assert_eq!(counters.reach_calculations, 1);

            // Baseline just above the upper reach edge: culled.
            let above = point(px(40.), clip.top() - margin - to_baseline - px(0.5));
            let (sprites, counters) = paint_clipped(window, cx, above);
            assert_eq!(sprites, 0);
            assert_eq!(counters.atlas_accesses + counters.raster_bounds_queries, 0);

            // Left of the left reach edge (glyph x at most L - (bw + 2)): the last glyph is
            // still 10 glyph advances (0.6 em) from the first, so push the whole line past it.
            let far_left = point(px(-2000.), px(300.) - to_baseline);
            let (sprites, counters) = paint_clipped(window, cx, far_left);
            assert_eq!(sprites, 0);
            assert_eq!(counters.atlas_accesses + counters.raster_bounds_queries, 0);
            let far_right = point(px(2000.), px(300.) - to_baseline);
            let (sprites, counters) = paint_clipped(window, cx, far_right);
            assert_eq!(sprites, 0);
            assert_eq!(counters.atlas_accesses + counters.raster_bounds_queries, 0);

            // An empty content mask rejects every glyph.
            window.reset_glyph_paint_counters();
            let before = window.next_frame.scene.monochrome_sprites.len();
            window.with_content_mask(
                Some(ContentMask {
                    bounds: bounds(point(px(100.), px(100.)), size(px(0.), px(50.))),
                }),
                |window| paint(&line, point(px(100.), px(100.)), line_height, window, cx),
            );
            assert_eq!(window.next_frame.scene.monochrome_sprites.len(), before);
            let counters = window.glyph_paint_counters();
            assert_eq!(counters.atlas_accesses + counters.raster_bounds_queries, 0);
        });
    }

    #[test]
    fn line_without_background_decoration_runs_no_background_pass() {
        run_paint(2.0, |window, cx| {
            let origin = point(px(40.), px(40.));
            let background = hsla(0.1, 1., 0.5, 1.);

            let plain = shape(
                window,
                "abcdefgh",
                16.,
                &[(4, black(), None), (4, hsla(0.5, 1., 0.5, 1.), None)],
            );
            let operations = window.next_frame.scene.paint_operations.len();
            let quads = window.next_frame.scene.quads.len();
            plain
                .paint_background(origin, px(20.), TextAlign::Left, None, window, cx)
                .unwrap();
            assert_eq!(
                window.next_frame.scene.paint_operations.len(),
                operations,
                "no layer, no quad"
            );
            assert_eq!(window.next_frame.scene.quads.len(), quads);

            let decorated = shape(
                window,
                "abcdefgh",
                16.,
                &[(4, black(), None), (4, black(), Some(background))],
            );
            decorated
                .paint_background(origin, px(20.), TextAlign::Left, None, window, cx)
                .unwrap();
            assert!(window.next_frame.scene.paint_operations.len() > operations);
            assert_eq!(window.next_frame.scene.quads.len(), quads + 1);
        });
    }
}
