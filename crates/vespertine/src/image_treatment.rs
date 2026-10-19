//! Cached source-space image treatments (ELM-005).
//!
//! A decorative image (a wallpaper, hero artwork) can be shown unchanged or
//! through one of four pixel treatments: scanlines, ordered color dither,
//! halftone dots or a text-density raster. A treatment is computed once per
//! source image and effect on the background executor, in the image's own
//! pixel space, and cached by source, effect and appearance: never by the size
//! or crop the image is displayed at. Crop, scaling and alpha masks are applied
//! at paint time by the caller.
//!
//! * [`SourceProxy`] is the 8-bit straight-alpha BGRA proxy (at most
//!   2048 x 2048) with its BT.709 luminance plane, and
//!   [`SourceProxy::treated_bgra`] runs the pure pixel math.
//! * [`TreatmentCache`] holds up to four sources, de-duplicates pending jobs and
//!   hands out shared [`RenderImage`] objects. [`PreloadedArtwork`] is the
//!   result of preloading one effect for a source.
//! * [`CrossfadeTracker`] keeps the current and previous images and drives the
//!   180 ms blend between them.
//!
//! Treated pixels are stored in blue, green, red, alpha order, so painting them
//! at 1:1 with nearest-neighbor sampling reproduces them exactly.

use crate::{App, Image, ImageFormat, RenderImage, SvgRenderer};
use anyhow::{Context as _, Result, anyhow};
use image::{Frame, ImageBuffer, imageops};
use smallvec::SmallVec;
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

/// The largest proxy dimension; larger sources are reduced to fit.
pub const PROXY_BOUND: u32 = 2048;
/// The number of decoded sources a [`TreatmentCache`] keeps.
pub const SOURCE_CACHE_CAPACITY: usize = 4;
/// The duration of the crossfade between two images.
pub const CROSSFADE_DURATION: Duration = Duration::from_millis(180);
/// The number of glyphs of a [`GlyphPalette`].
pub const GLYPH_COUNT: usize = 10;
/// The approximate number of colors [`SourceProxy::sampled_colors`] returns.
pub const COLOR_SAMPLE_TARGET: usize = 4096;

const TEXT_CELL_WIDTH: usize = 6;
const TEXT_CELL_HEIGHT: usize = 8;
const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;

/// A pixel treatment applied to a source image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageTreatment {
    /// The proxy, unchanged.
    None,
    /// Every third row is dimmed (dark paper) or lightened (light paper).
    Scanlines,
    /// Ordered color dither over 2 x 2 cells; independent of appearance.
    Dither,
    /// Halftone dots over 4 x 4 cells.
    Halftone,
    /// A raster of caller-supplied glyphs chosen by sampled density.
    TextDensity,
}

/// Whether the artwork sits on dark or light paper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TreatmentAppearance {
    /// Dark paper (black).
    Dark,
    /// Light paper (white).
    Light,
}

/// The identity of one treated variant of a source: the effect and the light
/// flag, which is always false for effects that ignore appearance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TreatmentKey {
    treatment: ImageTreatment,
    light: bool,
}

impl TreatmentKey {
    /// Normalizes the appearance: [`ImageTreatment::None`] and
    /// [`ImageTreatment::Dither`] do not depend on it, so both appearances share
    /// one entry.
    pub fn new(treatment: ImageTreatment, appearance: TreatmentAppearance) -> Self {
        let light = appearance == TreatmentAppearance::Light
            && !matches!(treatment, ImageTreatment::None | ImageTreatment::Dither);
        Self { treatment, light }
    }

    /// The effect.
    pub fn treatment(&self) -> ImageTreatment {
        self.treatment
    }

    /// The normalized light flag.
    pub fn light(&self) -> bool {
        self.light
    }
}

/// A palette row has bits outside the five-column ink area.
#[derive(Debug, thiserror::Error)]
#[error("glyph {glyph} row {row} has bits outside the 5 x 7 ink area: {bits:#010b}")]
pub struct GlyphPaletteError {
    glyph: usize,
    row: usize,
    bits: u8,
}

/// Ten 5 x 7 glyph bitmaps ordered by increasing ink density, supplied by the
/// caller for [`ImageTreatment::TextDensity`]. Each glyph has seven rows of five
/// bits, the most significant of the five bits being the leftmost column. No
/// glyph artwork is bundled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlyphPalette {
    glyphs: [[u8; GLYPH_HEIGHT]; GLYPH_COUNT],
}

impl GlyphPalette {
    /// Builds a palette; rejects rows that use bits above the fifth.
    pub fn new(glyphs: [[u8; GLYPH_HEIGHT]; GLYPH_COUNT]) -> Result<Self, GlyphPaletteError> {
        for (glyph, rows) in glyphs.iter().enumerate() {
            for (row, bits) in rows.iter().enumerate() {
                if bits & !0b1_1111 != 0 {
                    return Err(GlyphPaletteError {
                        glyph,
                        row,
                        bits: *bits,
                    });
                }
            }
        }
        Ok(Self { glyphs })
    }

    /// The algorithmically generated reproducible palette: glyph 0 is blank and
    /// glyph n (1 to 9) has its first min(4n, 35) ink-area pixels, in row-major
    /// order over the 5 x 7 area, set.
    pub fn ink_ramp() -> Self {
        let mut glyphs = [[0u8; GLYPH_HEIGHT]; GLYPH_COUNT];
        for (n, rows) in glyphs.iter_mut().enumerate().skip(1) {
            for pixel in 0..(4 * n).min(GLYPH_WIDTH * GLYPH_HEIGHT) {
                rows[pixel / GLYPH_WIDTH] |= 1 << (GLYPH_WIDTH - 1 - pixel % GLYPH_WIDTH);
            }
        }
        Self { glyphs }
    }

    /// The rows of glyph `index` (0 to 9).
    pub fn glyph(&self, index: usize) -> &[u8; GLYPH_HEIGHT] {
        &self.glyphs[index]
    }

    fn is_ink(&self, glyph: usize, u: usize, v: usize) -> bool {
        u < GLYPH_WIDTH
            && v < GLYPH_HEIGHT
            && self.glyphs[glyph][v] >> (GLYPH_WIDTH - 1 - u) & 1 == 1
    }
}

/// The glyph index for a density of 0 to 255: truncate(sqrt(density / 255) x 9),
/// at most 9. Evaluated exactly with integers (k is the largest value with
/// 255 k^2 <= 81 density).
pub fn glyph_index(density: u8) -> usize {
    (0..GLYPH_COUNT)
        .rev()
        .find(|k| 255 * k * k <= 81 * density as usize)
        .unwrap_or(0)
}

/// The 8-bit straight-alpha BGRA proxy of a decoded source, at most
/// [`PROXY_BOUND`] pixels on each side, with its BT.709 luminance plane.
pub struct SourceProxy {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    luminance: Vec<u8>,
}

impl SourceProxy {
    /// Wraps tightly packed BGRA pixels without reducing them. Returns `None`
    /// for a zero dimension or a buffer of the wrong length.
    pub fn from_bgra(width: u32, height: u32, bgra: Vec<u8>) -> Option<Self> {
        let len = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if width == 0 || height == 0 || bgra.len() != len {
            return None;
        }
        let luminance = bgra
            .chunks_exact(4)
            .map(|p| luminance(p[2], p[1], p[0]))
            .collect();
        Some(Self {
            width,
            height,
            bgra,
            luminance,
        })
    }

    /// Builds the proxy of a decoded image (its first frame), reducing it to fit
    /// within 2048 x 2048 pixels with the aspect ratio preserved. An image
    /// already within the bound is not enlarged.
    pub fn from_render_image(image: &RenderImage) -> Option<Self> {
        let size = image.size(0);
        let (width, height) = (
            u32::try_from(size.width.0).ok()?,
            u32::try_from(size.height.0).ok()?,
        );
        let bytes = image.as_bytes(0)?;
        let (target_width, target_height) = fit_within(width, height, PROXY_BOUND);
        if (target_width, target_height) == (width, height) {
            return Self::from_bgra(width, height, bytes.to_vec());
        }
        // The channels are resampled independently, so the byte order is irrelevant.
        let source: ImageBuffer<image::Rgba<u8>, &[u8]> =
            ImageBuffer::from_raw(width, height, bytes)?;
        let reduced = imageops::resize(
            &source,
            target_width,
            target_height,
            imageops::FilterType::Triangle,
        );
        Self::from_bgra(target_width, target_height, reduced.into_raw())
    }

    /// The width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The straight-alpha pixels in blue, green, red, alpha order.
    pub fn bgra(&self) -> &[u8] {
        &self.bgra
    }

    /// The BT.709 luminance of every pixel (alpha ignored).
    pub fn luminance(&self) -> &[u8] {
        &self.luminance
    }

    /// A strided sample of the proxy's colors as `[r, g, b, a]` (straight
    /// alpha): every n-th pixel with n = max(1, pixel count / 4096), about 4096
    /// samples, for palette derivation.
    pub fn sampled_colors(&self) -> Vec<[u8; 4]> {
        let pixels = self.width as usize * self.height as usize;
        let stride = (pixels / COLOR_SAMPLE_TARGET).max(1);
        self.bgra
            .chunks_exact(4)
            .step_by(stride)
            .map(|p| [p[2], p[1], p[0], p[3]])
            .collect()
    }

    fn pixel(&self, x: usize, y: usize) -> &[u8] {
        let index = (y * self.width as usize + x) * 4;
        &self.bgra[index..index + 4]
    }

    /// Runs one treatment and returns the treated pixels (blue, green, red,
    /// alpha) with exactly the proxy's dimensions. `light` selects light paper
    /// where the treatment depends on appearance.
    pub fn treated_bgra(
        &self,
        treatment: ImageTreatment,
        light: bool,
        palette: &GlyphPalette,
    ) -> Vec<u8> {
        match treatment {
            ImageTreatment::None => self.bgra.clone(),
            ImageTreatment::Scanlines => self.scanlines(light),
            ImageTreatment::Dither => self.dither(),
            ImageTreatment::Halftone => self.halftone(light),
            ImageTreatment::TextDensity => self.text_density(light, palette),
        }
    }

    /// Every third row (y mod 3 = 0) has gain 0.52: dark paper multiplies the
    /// channels by it, light paper moves them 48% toward white. Truncating.
    fn scanlines(&self, light: bool) -> Vec<u8> {
        let mut out = self.bgra.clone();
        let row_bytes = self.width as usize * 4;
        for (y, row) in out.chunks_exact_mut(row_bytes).enumerate() {
            if y % 3 != 0 {
                continue;
            }
            for pixel in row.chunks_exact_mut(4) {
                for channel in &mut pixel[..3] {
                    let c = *channel as u32;
                    *channel = if light {
                        c + (255 - c) * 48 / 100
                    } else {
                        c * 52 / 100
                    } as u8;
                }
            }
        }
        out
    }

    /// 2 x 2 cells sampled at their lower-right pixel and thresholded against
    /// the 4 x 4 ordered-dither matrix; ignores appearance.
    fn dither(&self) -> Vec<u8> {
        const MATRIX: [[u32; 4]; 4] =
            [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        let (width, height) = (self.width as usize, self.height as usize);
        let mut out = vec![0u8; self.bgra.len()];
        for cell_y in (0..height).step_by(2) {
            for cell_x in (0..width).step_by(2) {
                let sample = self.pixel((cell_x + 1).min(width - 1), (cell_y + 1).min(height - 1));
                let peak = sample[0].max(sample[1]).max(sample[2]) as u32;
                let entry = MATRIX[(cell_y / 2) % 4][(cell_x / 2) % 4];
                // peak / 255 > (entry + 0.5) / 16
                let bright = 32 * peak > (2 * entry + 1) * 255;
                let mut result = [0u8, 0, 0, sample[3]];
                for (out_channel, channel) in result.iter_mut().zip(&sample[..3]) {
                    let c = *channel as u32;
                    // Rounded to nearest: c * 255 / peak, or c * 0.08.
                    let scaled = if bright {
                        (2 * c * 255 + peak) / (2 * peak)
                    } else {
                        (4 * c + 25) / 50
                    };
                    *out_channel = scaled.min(255) as u8;
                }
                for y in cell_y..(cell_y + 2).min(height) {
                    for x in cell_x..(cell_x + 2).min(width) {
                        let index = (y * width + x) * 4;
                        out[index..index + 4].copy_from_slice(&result);
                    }
                }
            }
        }
        out
    }

    /// 4 x 4 cells with a dot whose radius grows with the cell luminance
    /// (inverted on light paper), blended 60% source and 40% dot or paper.
    fn halftone(&self, light: bool) -> Vec<u8> {
        let (width, height) = (self.width as usize, self.height as usize);
        let paper = if light { 255.0 } else { 0.0 };
        let mut out = vec![0u8; self.bgra.len()];
        for cell_y in (0..height).step_by(4) {
            for cell_x in (0..width).step_by(4) {
                let mut l = self.luminance[cell_y * width + cell_x];
                if light {
                    l = 255 - l;
                }
                let density = l as f64 / 255.0;
                let radius = 2.0 * (0.3 + 0.7 * density.sqrt());
                let dot = self.pixel((cell_x + 2).min(width - 1), (cell_y + 2).min(height - 1));
                let dot_alpha = dot[3] as f64 / 255.0;
                for dy in 0..4 {
                    for dx in 0..4 {
                        let (x, y) = (cell_x + dx, cell_y + dy);
                        if x >= width || y >= height {
                            continue;
                        }
                        let distance =
                            ((dx as f64 - 1.5).powi(2) + (dy as f64 - 1.5).powi(2)).sqrt();
                        let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0) * dot_alpha;
                        let source = self.pixel(x, y);
                        let index = (y * width + x) * 4;
                        for channel in 0..3 {
                            let blended = dot[channel] as f64 * coverage + paper * (1.0 - coverage);
                            // 0.60 source + 0.40 blend, truncated.
                            out[index + channel] = ((6.0 * source[channel] as f64 + 4.0 * blended)
                                / 10.0)
                                .floor() as u8;
                        }
                        out[index + 3] = source[3];
                    }
                }
            }
        }
        out
    }

    /// 6 x 8 cells of which a 5 x 7 ink area is drawn with the glyph chosen by
    /// the sampled density, blended 60% source and 40% sampled color or paper.
    fn text_density(&self, light: bool, palette: &GlyphPalette) -> Vec<u8> {
        let (width, height) = (self.width as usize, self.height as usize);
        let paper = if light { 255u32 } else { 0 };
        let mut out = vec![0u8; self.bgra.len()];
        for cell_y in (0..height).step_by(TEXT_CELL_HEIGHT) {
            for cell_x in (0..width).step_by(TEXT_CELL_WIDTH) {
                let sample_x = (cell_x + 3).min(width - 1);
                let sample_y = (cell_y + 4).min(height - 1);
                let sample = self.pixel(sample_x, sample_y);
                let l = self.luminance[sample_y * width + sample_x];
                let glyph = glyph_index(if light { 255 - l } else { l });
                for v in 0..TEXT_CELL_HEIGHT {
                    for u in 0..TEXT_CELL_WIDTH {
                        let (x, y) = (cell_x + u, cell_y + v);
                        if x >= width || y >= height {
                            continue;
                        }
                        let ink = palette.is_ink(glyph, u, v);
                        let source = self.pixel(x, y);
                        let index = (y * width + x) * 4;
                        for channel in 0..3 {
                            let overlay = if ink { sample[channel] as u32 } else { paper };
                            out[index + channel] =
                                ((6 * source[channel] as u32 + 4 * overlay) / 10) as u8;
                        }
                        out[index + 3] = source[3];
                    }
                }
            }
        }
        out
    }
}

/// ITU-R BT.709 luminance 0.2126 R + 0.7152 G + 0.0722 B rounded to 8 bits.
fn luminance(r: u8, g: u8, b: u8) -> u8 {
    ((2126 * r as u32 + 7152 * g as u32 + 722 * b as u32 + 5000) / 10000) as u8
}

/// The dimensions of an image of `width` x `height` reduced to fit within
/// `bound` x `bound` with the aspect ratio preserved; never enlarged.
fn fit_within(width: u32, height: u32, bound: u32) -> (u32, u32) {
    if width <= bound && height <= bound {
        return (width, height);
    }
    let scale = bound as f64 / width.max(height) as f64;
    let scaled = |side: u32| ((side as f64 * scale).round() as u32).clamp(1, bound);
    (scaled(width), scaled(height))
}

/// Wraps BGRA pixels (blue, green, red, alpha) as a single-frame render image.
///
/// This is the seam for the raw BGRA constructor of RND-007: the frame buffer
/// of a [`RenderImage`] already stores blue, green, red, alpha bytes.
fn bgra_render_image(width: u32, height: u32, bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buffer = ImageBuffer::from_raw(width, height, bgra)?;
    Some(Arc::new(RenderImage::new(SmallVec::from_elem(
        Frame::new(buffer),
        1,
    ))))
}

/// Detects an image format from its leading bytes (never from a file
/// extension): PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, PNM, then SVG, in that order.
fn sniff_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else if bytes.starts_with(b"BM") {
        Some(ImageFormat::Bmp)
    } else if bytes.starts_with(&[0x49, 0x49, 0x2A, 0x00])
        || bytes.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
    {
        Some(ImageFormat::Tiff)
    } else if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        Some(ImageFormat::Ico)
    } else if bytes.len() >= 2 && bytes[0] == b'P' && (b'1'..=b'6').contains(&bytes[1]) {
        Some(ImageFormat::Pnm)
    } else {
        let text = bytes.trim_ascii_start();
        (text.starts_with(b"<svg") || text.starts_with(b"<?xml")).then_some(ImageFormat::Svg)
    }
}

/// Decodes image bytes of any supported format after sniffing the actual bytes.
fn decode_image_bytes(bytes: &[u8], svg_renderer: &SvgRenderer) -> Result<Arc<RenderImage>> {
    let format = sniff_image_format(bytes).ok_or_else(|| anyhow!("unrecognized image format"))?;
    Image::from_bytes(format, bytes.to_vec()).to_image_data(svg_renderer.clone())
}

/// The result of preloading one effect of a source: the reduced proxy and the
/// treated image, ready to be installed into a [`TreatmentCache`].
pub struct PreloadedArtwork {
    proxy: Arc<SourceProxy>,
    key: TreatmentKey,
    image: Arc<RenderImage>,
}

impl PreloadedArtwork {
    /// Builds the proxy and the treated image of an already decoded image, so a
    /// full-resolution image decoded to validate a candidate is not decoded a
    /// second time. Intended to run on a background thread.
    pub fn new(
        decoded: &RenderImage,
        treatment: ImageTreatment,
        appearance: TreatmentAppearance,
        palette: &GlyphPalette,
    ) -> Result<Self> {
        let proxy =
            SourceProxy::from_render_image(decoded).context("the decoded image has no pixels")?;
        Self::from_proxy(
            Arc::new(proxy),
            TreatmentKey::new(treatment, appearance),
            palette,
        )
    }

    fn from_proxy(
        proxy: Arc<SourceProxy>,
        key: TreatmentKey,
        palette: &GlyphPalette,
    ) -> Result<Self> {
        let image = treated_image(&proxy, key, palette)?;
        Ok(Self { proxy, key, image })
    }

    /// The proxy all treatments of this source use.
    pub fn proxy(&self) -> &Arc<SourceProxy> {
        &self.proxy
    }

    /// The treated image.
    pub fn image(&self) -> &Arc<RenderImage> {
        &self.image
    }

    /// A strided sample of the proxy's colors; see [`SourceProxy::sampled_colors`].
    pub fn sampled_colors(&self) -> Vec<[u8; 4]> {
        self.proxy.sampled_colors()
    }
}

fn treated_image(
    proxy: &SourceProxy,
    key: TreatmentKey,
    palette: &GlyphPalette,
) -> Result<Arc<RenderImage>> {
    let pixels = proxy.treated_bgra(key.treatment, key.light, palette);
    bgra_render_image(proxy.width, proxy.height, pixels)
        .context("treated pixels do not match the proxy")
}

struct CachedSource {
    id: Arc<Path>,
    proxy: Arc<SourceProxy>,
    treated: Vec<(TreatmentKey, Arc<RenderImage>)>,
}

struct CacheState {
    palette: Arc<GlyphPalette>,
    /// Oldest-inserted first.
    sources: VecDeque<CachedSource>,
    pending: Vec<(Arc<Path>, TreatmentKey)>,
    failed: Vec<Arc<Path>>,
    jobs_started: u64,
}

impl CacheState {
    fn source(&self, id: &Path) -> Option<&CachedSource> {
        self.sources.iter().find(|source| &*source.id == id)
    }

    fn insert(&mut self, source: CachedSource) {
        self.sources.retain(|existing| existing.id != source.id);
        while self.sources.len() >= SOURCE_CACHE_CAPACITY {
            self.sources.pop_front();
        }
        self.sources.push_back(source);
    }
}

/// The output of a background treatment job.
struct JobOutput {
    proxy: Arc<SourceProxy>,
    image: Arc<RenderImage>,
}

/// A cache of treated images keyed by (source identity, effect, light flag).
///
/// Holds at most [`SOURCE_CACHE_CAPACITY`] decoded sources and evicts the
/// oldest-inserted one when a fifth arrives. Cloning shares the cache.
#[derive(Clone)]
pub struct TreatmentCache(Rc<std::cell::RefCell<CacheState>>);

impl TreatmentCache {
    /// Creates a cache whose text-density treatment uses `palette`.
    pub fn new(palette: GlyphPalette) -> Self {
        Self(Rc::new(std::cell::RefCell::new(CacheState {
            palette: Arc::new(palette),
            sources: VecDeque::new(),
            pending: Vec::new(),
            failed: Vec::new(),
            jobs_started: 0,
        })))
    }

    /// Returns the shared treated image when it is ready. Otherwise returns
    /// `None` immediately, marks exactly this (source, effect, light flag) as
    /// pending so concurrent requests start no second job, decodes the source
    /// (a file path) and generates the treatment on the background executor,
    /// stores the image and requests a window refresh when it finishes.
    ///
    /// A source that cannot be read or decoded yields `None` and is not retried
    /// until [`Self::forget`] is called for it. Nothing about the viewport, crop
    /// or alpha mask enters the key.
    pub fn request(
        &self,
        treatment: ImageTreatment,
        source: impl AsRef<Path>,
        appearance: TreatmentAppearance,
        cx: &mut App,
    ) -> Option<Arc<RenderImage>> {
        let source = source.as_ref();
        let key = TreatmentKey::new(treatment, appearance);
        let (id, cached_proxy, palette) = {
            let mut state = self.0.borrow_mut();
            if let Some(cached) = state.source(source) {
                if let Some((_, image)) = cached.treated.iter().find(|(k, _)| *k == key) {
                    return Some(image.clone());
                }
            }
            if state.failed.iter().any(|failed| &**failed == source)
                || state
                    .pending
                    .iter()
                    .any(|(pending, k)| &**pending == source && *k == key)
            {
                return None;
            }
            let id: Arc<Path> = Arc::from(source);
            state.pending.push((id.clone(), key));
            state.jobs_started += 1;
            let cached_proxy = state.source(source).map(|cached| cached.proxy.clone());
            (id, cached_proxy, state.palette.clone())
        };

        let renderer = cx.svg_renderer();
        let work = cx.background_executor().spawn({
            let id = id.clone();
            async move { run_job(&id, cached_proxy, key, &palette, &renderer) }
        });
        let this = self.clone();
        cx.spawn(async move |cx| {
            let result = work.await;
            cx.update(|cx| this.finish(id, key, result, cx));
        })
        .detach();
        None
    }

    fn finish(&self, id: Arc<Path>, key: TreatmentKey, result: Result<JobOutput>, cx: &mut App) {
        let mut state = self.0.borrow_mut();
        state
            .pending
            .retain(|(pending, k)| !(*pending == id && *k == key));
        match result {
            Ok(JobOutput { proxy, image }) => {
                if let Some(position) = state.sources.iter().position(|s| s.id == id) {
                    let treated = &mut state.sources[position].treated;
                    if !treated.iter().any(|(k, _)| *k == key) {
                        treated.push((key, image));
                    }
                } else {
                    state.insert(CachedSource {
                        id,
                        proxy,
                        treated: vec![(key, image)],
                    });
                }
                drop(state);
                cx.refresh_windows();
            }
            Err(error) => {
                log::error!("image treatment of {} failed: {error:#}", id.display());
                if !state.failed.contains(&id) {
                    state.failed.push(id);
                }
            }
        }
    }

    /// Installs a preloaded source under `source`, replacing any entry with the
    /// same identity and honoring the limit of four sources.
    pub fn install(&self, source: impl AsRef<Path>, artwork: PreloadedArtwork) {
        let id: Arc<Path> = Arc::from(source.as_ref());
        let mut state = self.0.borrow_mut();
        state.failed.retain(|failed| *failed != id);
        state.insert(CachedSource {
            id,
            proxy: artwork.proxy,
            treated: vec![(artwork.key, artwork.image)],
        });
    }

    /// Decodes `source` and generates one effect fully on the background
    /// executor, then installs the pair so the first display after selection does
    /// not wait. The task resolves to the artwork's sampled colors.
    pub fn preload_file(
        &self,
        source: impl AsRef<Path>,
        treatment: ImageTreatment,
        appearance: TreatmentAppearance,
        cx: &mut App,
    ) -> crate::Task<Result<Vec<[u8; 4]>>> {
        let source: PathBuf = source.as_ref().to_path_buf();
        let palette = self.0.borrow().palette.clone();
        let renderer = cx.svg_renderer();
        let work = cx.background_executor().spawn({
            let source = source.clone();
            async move {
                let decoded = decode_file(&source, &renderer)?;
                PreloadedArtwork::new(&decoded, treatment, appearance, &palette)
            }
        });
        let this = self.clone();
        cx.spawn(async move |_| {
            let artwork = work.await?;
            let colors = artwork.sampled_colors();
            this.install(&source, artwork);
            Ok(colors)
        })
    }

    /// Like [`Self::preload_file`] for an image that is already decoded (for
    /// example one decoded to validate a candidate): it is not decoded again.
    pub fn preload_decoded(
        &self,
        source: impl AsRef<Path>,
        decoded: Arc<RenderImage>,
        treatment: ImageTreatment,
        appearance: TreatmentAppearance,
        cx: &mut App,
    ) -> crate::Task<Result<Vec<[u8; 4]>>> {
        let source: PathBuf = source.as_ref().to_path_buf();
        let palette = self.0.borrow().palette.clone();
        let work = cx
            .background_executor()
            .spawn(async move { PreloadedArtwork::new(&decoded, treatment, appearance, &palette) });
        let this = self.clone();
        cx.spawn(async move |_| {
            let artwork = work.await?;
            let colors = artwork.sampled_colors();
            this.install(&source, artwork);
            Ok(colors)
        })
    }

    /// Drops a source (and its failure record) so the next request starts over.
    pub fn forget(&self, source: impl AsRef<Path>) {
        let source = source.as_ref();
        let mut state = self.0.borrow_mut();
        state.sources.retain(|cached| &*cached.id != source);
        state.failed.retain(|failed| &**failed != source);
    }

    /// The cached proxy of a source, if it is cached.
    pub fn proxy(&self, source: impl AsRef<Path>) -> Option<Arc<SourceProxy>> {
        self.0
            .borrow()
            .source(source.as_ref())
            .map(|cached| cached.proxy.clone())
    }

    /// The cached sources, oldest-inserted first.
    pub fn cached_sources(&self) -> Vec<PathBuf> {
        self.0
            .borrow()
            .sources
            .iter()
            .map(|cached| cached.id.to_path_buf())
            .collect()
    }

    /// The number of background jobs started so far (a request that finds its
    /// image ready or pending starts none).
    pub fn jobs_started(&self) -> u64 {
        self.0.borrow().jobs_started
    }
}

fn decode_file(path: &Path, renderer: &SvgRenderer) -> Result<Arc<RenderImage>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    decode_image_bytes(&bytes, renderer)
}

fn run_job(
    id: &Path,
    cached_proxy: Option<Arc<SourceProxy>>,
    key: TreatmentKey,
    palette: &GlyphPalette,
    renderer: &SvgRenderer,
) -> Result<JobOutput> {
    let proxy = match cached_proxy {
        Some(proxy) => proxy,
        None => {
            let decoded = decode_file(id, renderer)?;
            let proxy = SourceProxy::from_render_image(&decoded)
                .ok_or_else(|| anyhow!("{} has no pixels", id.display()))?;
            Arc::new(proxy)
        }
    };
    let image = treated_image(&proxy, key, palette)?;
    Ok(JobOutput { proxy, image })
}

/// The current or previous image of a [`CrossfadeTracker`] with the framing
/// record (crop adjustment) and source identity it was adopted with.
#[derive(Clone)]
struct Slot<F, S> {
    image: Arc<RenderImage>,
    source: S,
    framing: F,
}

/// What to paint for one frame of a [`CrossfadeTracker`].
#[derive(Clone)]
pub struct CrossfadeFrame<F> {
    /// The image fading in (or shown), with its framing record.
    pub current: Option<(Arc<RenderImage>, F)>,
    /// The image fading out, with its framing record.
    pub previous: Option<(Arc<RenderImage>, F)>,
    /// The eased blend mix in [0, 1]: 1 when no blend is active.
    pub mix: f32,
    /// Whether a blend is active; the host then requests the next animation
    /// frame.
    pub active: bool,
}

impl<F> CrossfadeFrame<F> {
    /// The opacities to paint the (previous, current) layers with at the given
    /// base opacity: (1 - mix) x base and mix x base. Each layer goes through the
    /// same crop, masks and paint path, so at mix 0.5 both are at half of the
    /// base and the combined coverage dips below the base during the blend.
    pub fn layer_opacities(&self, base: f32) -> (f32, f32) {
        ((1.0 - self.mix) * base, self.mix * base)
    }
}

/// Holds the current and previous image of artwork and blends between them over
/// 180 ms when a different treated image becomes ready.
///
/// `F` is the caller's framing record (crop adjustment) and `S` the source
/// identity. Images are compared by identity (their [`RenderImage::id`]).
pub struct CrossfadeTracker<F, S> {
    current: Option<Slot<F, S>>,
    previous: Option<Slot<F, S>>,
    blend_start: Option<Instant>,
}

impl<F, S> Default for CrossfadeTracker<F, S> {
    fn default() -> Self {
        Self {
            current: None,
            previous: None,
            blend_start: None,
        }
    }
}

impl<F: Clone, S: Clone + PartialEq> CrossfadeTracker<F, S> {
    /// Creates an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Advances one frame.
    ///
    /// `treated` is the treated image if ready (`None` while `enabled` means it is
    /// still loading and the current image stays visible), `source` its source
    /// identity, `framing` the current framing record, `enabled` whether the
    /// artwork is shown at all, `reduced_motion` whether motion is reduced and
    /// `now` the frame time.
    pub fn frame(
        &mut self,
        treated: Option<&Arc<RenderImage>>,
        source: &S,
        framing: &F,
        enabled: bool,
        reduced_motion: bool,
        now: Instant,
    ) -> CrossfadeFrame<F> {
        // Progress of an active blend; a finished blend drops the previous image.
        let mut mix = 1.0;
        if let Some(start) = self.blend_start {
            let elapsed = now.saturating_duration_since(start);
            let t = (elapsed.as_secs_f32() / CROSSFADE_DURATION.as_secs_f32()).clamp(0.0, 1.0);
            mix = 1.0 - (1.0 - t).powi(3);
            if reduced_motion || t >= 1.0 {
                self.previous = None;
                self.blend_start = None;
                mix = 1.0;
            }
        }

        // Framing belongs to the source: while a new treatment of the same
        // source is loading, the supplied framing replaces the image's framing
        // immediately and nothing fades.
        if enabled {
            if let Some(current) = self.current.as_mut().filter(|c| c.source == *source) {
                current.framing = framing.clone();
            } else if treated.is_none() {
                if let Some(previous) = self.previous.as_mut().filter(|p| p.source == *source) {
                    previous.framing = framing.clone();
                }
            }
        }

        // Adoption happens only between blends, and only for a ready image (or
        // when the artwork is disabled).
        if self.blend_start.is_none() && (!enabled || treated.is_some()) {
            let target = if enabled { treated } else { None };
            let differs = match (&self.current, target) {
                (Some(current), Some(target)) => current.image.id != target.id,
                (None, None) => false,
                _ => true,
            };
            if differs {
                let adopted = target.map(|image| Slot {
                    image: image.clone(),
                    source: source.clone(),
                    framing: framing.clone(),
                });
                if reduced_motion {
                    self.previous = None;
                    self.current = adopted;
                } else {
                    self.previous = self.current.take();
                    self.current = adopted;
                    self.blend_start = Some(now);
                    mix = 0.0;
                }
            }
        }

        let slot = |slot: &Option<Slot<F, S>>| {
            slot.as_ref()
                .map(|slot| (slot.image.clone(), slot.framing.clone()))
        };
        CrossfadeFrame {
            current: slot(&self.current),
            previous: slot(&self.previous),
            mix,
            active: self.blend_start.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestAppContext;
    use std::cell::RefCell;

    /// Builds a proxy from a closure returning `[r, g, b, a]` per pixel.
    fn proxy(width: u32, height: u32, pixel: impl Fn(u32, u32) -> [u8; 4]) -> SourceProxy {
        let mut bgra = Vec::new();
        for y in 0..height {
            for x in 0..width {
                let [r, g, b, a] = pixel(x, y);
                bgra.extend([b, g, r, a]);
            }
        }
        SourceProxy::from_bgra(width, height, bgra).unwrap()
    }

    /// The `[r, g, b, a]` of a treated pixel.
    fn at(bgra: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [bgra[i + 2], bgra[i + 1], bgra[i], bgra[i + 3]]
    }

    fn treat(proxy: &SourceProxy, treatment: ImageTreatment, light: bool) -> Vec<u8> {
        proxy.treated_bgra(treatment, light, &GlyphPalette::ink_ramp())
    }

    #[test]
    fn luminance_uses_bt709_weights() {
        let p = proxy(4, 1, |x, _| match x {
            0 => [255, 0, 0, 255],
            1 => [0, 255, 0, 255],
            2 => [0, 0, 255, 255],
            _ => [255, 255, 255, 0],
        });
        assert_eq!(p.luminance(), &[54, 182, 18, 255]);
    }

    #[test]
    fn proxy_fits_within_the_bound_without_enlarging() {
        assert_eq!(fit_within(100, 50, PROXY_BOUND), (100, 50));
        assert_eq!(fit_within(2048, 2048, PROXY_BOUND), (2048, 2048));
        assert_eq!(fit_within(4096, 1024, PROXY_BOUND), (2048, 512));
        assert_eq!(fit_within(1000, 6000, PROXY_BOUND), (341, 2048));
        assert_eq!(fit_within(10000, 1, PROXY_BOUND), (2048, 1));

        let big = bgra_render_image(4096, 1024, vec![200; 4096 * 1024 * 4]).unwrap();
        let reduced = SourceProxy::from_render_image(&big).unwrap();
        assert_eq!((reduced.width(), reduced.height()), (2048, 512));
        assert!(reduced.bgra().iter().all(|byte| (199..=201).contains(byte)));
        let small = bgra_render_image(3, 2, vec![7; 24]).unwrap();
        let kept = SourceProxy::from_render_image(&small).unwrap();
        assert_eq!(
            (kept.width(), kept.height(), kept.bgra()),
            (3, 2, &[7u8; 24][..])
        );
    }

    #[test]
    fn sampled_colors_are_strided() {
        let p = proxy(100, 100, |x, y| [(y * 100 + x) as u8, 0, 0, 255]);
        // 10000 pixels: stride 10000 / 4096 = 2.
        let colors = p.sampled_colors();
        assert_eq!(colors.len(), 5000);
        assert_eq!(colors[1], [2, 0, 0, 255]);
        let tiny = proxy(3, 1, |x, _| [x as u8, 0, 0, 255]);
        assert_eq!(tiny.sampled_colors().len(), 3);
    }

    #[test]
    fn scanlines_dim_every_third_row() {
        let p = proxy(2, 4, |_, _| [160, 96, 64, 255]);
        let dark = treat(&p, ImageTreatment::Scanlines, false);
        for x in 0..2 {
            assert_eq!(at(&dark, 2, x, 0), [83, 49, 33, 255]);
            assert_eq!(at(&dark, 2, x, 1), [160, 96, 64, 255]);
            assert_eq!(at(&dark, 2, x, 2), [160, 96, 64, 255]);
            assert_eq!(at(&dark, 2, x, 3), [83, 49, 33, 255]);
        }
        // Light paper moves the dimmed rows 48% toward white (truncating).
        let light = treat(&p, ImageTreatment::Scanlines, true);
        assert_eq!(at(&light, 2, 0, 0), [205, 172, 155, 255]);
        assert_eq!(at(&light, 2, 0, 1), [160, 96, 64, 255]);
        assert_eq!(at(&light, 2, 1, 3), [205, 172, 155, 255]);
    }

    #[test]
    fn dither_thresholds_follow_the_matrix() {
        // Matrix row 0: 0 8 2 10 (thresholds peak >= 8, 136, 40, 168);
        // row 1: 12 4 14 6 (peak >= 200, 72, 232, 104).
        let p = proxy(8, 4, |_, _| [150, 90, 30, 200]);
        let out = treat(&p, ImageTreatment::Dither, false);
        let bright = [255, 153, 51, 200];
        let dark = [12, 7, 2, 200];
        let expected = [
            [bright, bright, bright, dark],
            // Row 1 of the matrix: 12 (dark), 4 (bright), 14 (dark), 6 (bright).
            [dark, bright, dark, bright],
        ];
        for (cell_row, row) in expected.iter().enumerate() {
            for (cell, color) in row.iter().enumerate() {
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let (x, y) = (cell as u32 * 2 + dx, cell_row as u32 * 2 + dy);
                    assert_eq!(at(&out, 8, x, y), *color, "pixel ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn dither_rounds_to_nearest_and_wraps_the_matrix() {
        // Peak 200: bright everywhere; 100 * 255 / 200 = 127.5 rounds to 128,
        // 50 * 255 / 200 = 63.75 to 64.
        let bright = treat(
            &proxy(2, 2, |_, _| [200, 100, 50, 255]),
            ImageTreatment::Dither,
            false,
        );
        assert_eq!(at(&bright, 2, 0, 0), [255, 128, 64, 255]);
        // Peak 100 is dark in matrix column 1 (threshold peak >= 136): 60 * 0.08 =
        // 4.8 rounds to 5 and 20 * 0.08 = 1.6 to 2, and in column 0 it is bright.
        let p = proxy(4, 2, |_, _| [100, 60, 20, 255]);
        let out = treat(&p, ImageTreatment::Dither, false);
        assert_eq!(at(&out, 4, 0, 0), [255, 153, 51, 255]);
        assert_eq!(at(&out, 4, 2, 1), [8, 5, 2, 255]);
        // A tenth column of cells reuses matrix column 0 (cell x 4 mod 4).
        let wide = treat(
            &proxy(10, 2, |_, _| [100, 60, 20, 255]),
            ImageTreatment::Dither,
            false,
        );
        assert_eq!(at(&wide, 10, 8, 0), [255, 153, 51, 255]);
        assert_eq!(at(&wide, 10, 9, 1), [255, 153, 51, 255]);
    }

    #[test]
    fn dither_samples_the_lower_right_pixel_clamped_and_keeps_its_alpha() {
        // Rows differ in alpha; the sample of cell (0, 0) is pixel (1, 1).
        let p = proxy(2, 2, |x, y| match (x, y) {
            (1, 1) => [200, 0, 0, 77],
            _ => [0, 255, 0, 255],
        });
        let out = treat(&p, ImageTreatment::Dither, false);
        for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            assert_eq!(at(&out, 2, x, y), [255, 0, 0, 77]);
        }
        // The last cell of a 3 x 1 image samples (min(3, 2), min(1, 0)).
        let p = proxy(3, 1, |x, _| {
            [[10, 10, 10, 255], [200, 0, 0, 255], [0, 0, 220, 90]][x as usize]
        });
        let out = treat(&p, ImageTreatment::Dither, false);
        // Cell 0 samples pixel (1, 0): bright (peak 200 >= 8): (255, 0, 0).
        assert_eq!(at(&out, 3, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&out, 3, 1, 0), [255, 0, 0, 255]);
        // Cell 1 (x = 2) samples the clamped pixel (2, 0); matrix entry 8 needs peak >= 136.
        assert_eq!(at(&out, 3, 2, 0), [0, 0, 255, 90]);
    }

    #[test]
    fn dither_ignores_appearance() {
        let p = proxy(8, 8, |x, y| {
            [(x * 30) as u8, (y * 30) as u8, (x * y) as u8, 255]
        });
        assert_eq!(
            treat(&p, ImageTreatment::Dither, false),
            treat(&p, ImageTreatment::Dither, true)
        );
    }

    #[test]
    fn halftone_dark_density_zero() {
        // The cell luminance is read at the cell origin (black: density 0), the dot
        // color at (2, 2); every other pixel is grey 100.
        let p = proxy(4, 4, |x, y| match (x, y) {
            (0, 0) => [0, 0, 0, 255],
            (2, 2) => [200, 100, 50, 255],
            _ => [100, 100, 100, 255],
        });
        let out = treat(&p, ImageTreatment::Halftone, false);
        // Radius 2 x 0.3 = 0.6. A central pixel (distance 0.7071) has coverage
        // 0.6 + 0.5 - 0.7071 = 0.3929: 0.60 x 100 + 0.40 x (c x 0.3929).
        assert_eq!(at(&out, 4, 1, 1), [91, 75, 67, 255]);
        // The dot pixel blends its own color: 0.6 x 200 + 0.4 x 78.58 = 151.43,
        // 0.6 x 100 + 0.4 x 39.29 = 75.7, 0.6 x 50 + 0.4 x 19.64 = 37.9.
        assert_eq!(at(&out, 4, 2, 2), [151, 75, 37, 255]);
        // A corner (distance 2.1213) has no coverage: 60% source + 40% black paper.
        assert_eq!(at(&out, 4, 3, 3), [60, 60, 60, 255]);
        assert_eq!(at(&out, 4, 0, 0), [0, 0, 0, 255]);
    }

    #[test]
    fn halftone_dark_density_one() {
        let p = proxy(4, 4, |x, y| match (x, y) {
            (0, 0) => [255, 255, 255, 255],
            (2, 2) => [200, 100, 50, 255],
            _ => [100, 100, 100, 255],
        });
        let out = treat(&p, ImageTreatment::Halftone, false);
        // Radius 2: full coverage at 0.7071, so 0.60 x source + 0.40 x dot.
        assert_eq!(at(&out, 4, 1, 1), [140, 100, 80, 255]);
        assert_eq!(at(&out, 4, 2, 2), [200, 100, 50, 255]);
        // Corner: coverage 2.5 - 2.1213 = 0.3787 (the 1 pixel ramp).
        assert_eq!(at(&out, 4, 3, 3), [90, 75, 67, 255]);
        // Pixels at distance 1.58 (dx 0, dy 1) get coverage 0.9189.
        // 0.6 x 100 + 0.4 x (200 x 0.9189) = 133.51.
        assert_eq!(at(&out, 4, 0, 1)[0], 133);
    }

    #[test]
    fn halftone_light_paper_inverts_luminance() {
        // White origin pixel: light-paper density 255 - 255 = 0, paper 255.
        let p = proxy(4, 4, |x, y| match (x, y) {
            (0, 0) => [255, 255, 255, 255],
            (2, 2) => [200, 100, 50, 255],
            _ => [100, 100, 100, 255],
        });
        let out = treat(&p, ImageTreatment::Halftone, true);
        // Coverage 0.3929 of the dot over (1 - 0.3929) paper 255.
        assert_eq!(at(&out, 4, 1, 1), [153, 137, 129, 255]);
        // Corner: no coverage, so 0.6 x 100 + 0.4 x 255 = 162.
        assert_eq!(at(&out, 4, 3, 3), [162, 162, 162, 255]);
    }

    #[test]
    fn halftone_dot_alpha_scales_coverage_and_output_alpha_is_the_sources() {
        let p = proxy(4, 4, |x, y| match (x, y) {
            (0, 0) => [255, 255, 255, 40],
            (2, 2) => [200, 200, 200, 0],
            _ => [100, 100, 100, 40],
        });
        let out = treat(&p, ImageTreatment::Halftone, false);
        // The dot is fully transparent: no coverage even where the dot would be, so
        // the blend is 60% source + 40% black paper; alpha is each pixel's own.
        assert_eq!(at(&out, 4, 1, 1), [60, 60, 60, 40]);
        assert_eq!(at(&out, 4, 2, 2), [120, 120, 120, 0]);
    }

    #[test]
    fn halftone_handles_partial_cells_at_the_edge() {
        let p = proxy(5, 5, |_, _| [100, 100, 100, 255]);
        let out = treat(&p, ImageTreatment::Halftone, false);
        assert_eq!(out.len(), 5 * 5 * 4);
        // The cell at (4, 4) has a single pixel; its dot pixel clamps to (4, 4).
        assert_eq!(at(&out, 5, 4, 4)[3], 255);
    }

    #[test]
    fn glyph_indices() {
        assert_eq!(glyph_index(0), 0);
        assert_eq!(glyph_index(3), 0);
        assert_eq!(glyph_index(4), 1);
        assert_eq!(glyph_index(64), 4);
        assert_eq!(glyph_index(128), 6);
        assert_eq!(glyph_index(254), 8);
        assert_eq!(glyph_index(255), 9);
        // Agrees with the floating point definition elsewhere.
        for density in 0..=255u32 {
            let float = ((density as f64 / 255.0).sqrt() * 9.0) as usize;
            assert_eq!(glyph_index(density as u8), float.min(9), "{density}");
        }
    }

    #[test]
    fn ink_ramp_palette() {
        let palette = GlyphPalette::ink_ramp();
        assert_eq!(palette.glyph(0), &[0; 7]);
        // Glyph 1: 4 pixels in the first row, the leftmost being the top bit.
        assert_eq!(palette.glyph(1), &[0b11110, 0, 0, 0, 0, 0, 0]);
        // Glyph 2: 8 pixels: a full row and three more.
        assert_eq!(palette.glyph(2), &[0b11111, 0b11100, 0, 0, 0, 0, 0]);
        // Glyph 9: min(36, 35) pixels: everything.
        assert_eq!(palette.glyph(9), &[0b11111; 7]);
        for n in 1..10 {
            let ink: u32 = palette.glyph(n).iter().map(|row| row.count_ones()).sum();
            assert_eq!(ink as usize, (4 * n).min(35));
        }
        assert!(GlyphPalette::new([[0; 7]; 10]).is_ok());
        let mut invalid = [[0; 7]; 10];
        invalid[3][2] = 0b10_0000;
        assert!(GlyphPalette::new(invalid).is_err());
    }

    /// A 6 x 8 cell of grey 100 whose sample pixel (3, 4) is `sample`.
    fn text_cell(sample: [u8; 4]) -> SourceProxy {
        proxy(6, 8, |x, y| {
            if (x, y) == (3, 4) {
                sample
            } else {
                [100, 100, 100, 255]
            }
        })
    }

    #[test]
    fn text_density_density_zero_draws_the_blank_glyph() {
        let out = treat(
            &text_cell([0, 0, 0, 255]),
            ImageTreatment::TextDensity,
            false,
        );
        for (x, y) in [(0, 0), (4, 6), (5, 0), (0, 7)] {
            // Paper black: 0.6 x 100.
            assert_eq!(at(&out, 6, x, y), [60, 60, 60, 255], "({x}, {y})");
        }
    }

    #[test]
    fn text_density_full_density_draws_the_full_glyph() {
        let out = treat(
            &text_cell([255, 255, 255, 255]),
            ImageTreatment::TextDensity,
            false,
        );
        // Ink: 0.6 x 100 + 0.4 x 255 = 162.
        assert_eq!(at(&out, 6, 0, 0), [162, 162, 162, 255]);
        assert_eq!(at(&out, 6, 4, 6), [162, 162, 162, 255]);
        // The sixth column and eighth row are spacing: paper.
        for (x, y) in [(5, 0), (5, 6), (0, 7), (4, 7), (5, 7)] {
            assert_eq!(at(&out, 6, x, y), [60, 60, 60, 255], "({x}, {y})");
        }
    }

    #[test]
    fn text_density_mid_density_selects_a_partial_glyph() {
        // Sample (200, 100, 50): luminance 118 selects glyph 6 (24 ink pixels: four
        // full rows and four pixels of the fifth).
        let sample = [200, 100, 50, 255];
        let out = treat(&text_cell(sample), ImageTreatment::TextDensity, false);
        assert_eq!(at(&out, 6, 0, 0), [140, 100, 80, 255]);
        assert_eq!(at(&out, 6, 4, 3), [140, 100, 80, 255]);
        assert_eq!(at(&out, 6, 0, 4), [140, 100, 80, 255]);
        assert_eq!(at(&out, 6, 3, 4), [200, 100, 50, 255]);
        // Pixel (4, 4) is the fifth pixel of the fifth row: not ink.
        assert_eq!(at(&out, 6, 4, 4), [60, 60, 60, 255]);
        assert_eq!(at(&out, 6, 0, 5), [60, 60, 60, 255]);
        assert_eq!(at(&out, 6, 5, 0), [60, 60, 60, 255]);
    }

    #[test]
    fn text_density_light_paper_inverts_density() {
        // White sample: light density 255 - 255 = 0, blank glyph, paper white.
        let out = treat(
            &text_cell([255, 255, 255, 255]),
            ImageTreatment::TextDensity,
            true,
        );
        // 0.6 x 100 + 0.4 x 255 = 162.
        assert_eq!(at(&out, 6, 0, 0), [162, 162, 162, 255]);
        // Black sample: density 255, full glyph, ink = the black sample.
        let out = treat(
            &text_cell([0, 0, 0, 255]),
            ImageTreatment::TextDensity,
            true,
        );
        assert_eq!(at(&out, 6, 0, 0), [60, 60, 60, 255]);
        assert_eq!(at(&out, 6, 5, 0), [162, 162, 162, 255]);
    }

    #[test]
    fn text_density_cells_tile_and_clamp_their_sample() {
        // 7 x 9: the second cell column (x = 6) and second row (y = 8) are partial;
        // their samples (9, 4), (3, 12), (9, 12) clamp to the last column / row.
        let p = proxy(7, 9, |x, y| {
            if (x, y) == (6, 8) {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        });
        let out = treat(&p, ImageTreatment::TextDensity, false);
        // The corner cell (6, 8) samples itself: density 255, glyph 9, pixel (6, 8) is ink.
        assert_eq!(at(&out, 7, 6, 8), [255, 255, 255, 255]);
        // The other cells sample black: blank glyph, zero everywhere.
        assert_eq!(at(&out, 7, 0, 0), [0, 0, 0, 255]);
        assert_eq!(out.len(), 7 * 9 * 4);
    }

    #[test]
    fn every_treatment_preserves_source_alpha() {
        for alpha in [0u8, 128, 255] {
            let p = proxy(13, 17, |x, y| {
                [(x * 19) as u8, (y * 15) as u8, (x * y) as u8, alpha]
            });
            for treatment in [
                ImageTreatment::None,
                ImageTreatment::Scanlines,
                ImageTreatment::Dither,
                ImageTreatment::Halftone,
                ImageTreatment::TextDensity,
            ] {
                for light in [false, true] {
                    let out = treat(&p, treatment, light);
                    assert_eq!(out.len(), p.bgra().len());
                    assert!(
                        out.chunks_exact(4).all(|pixel| pixel[3] == alpha),
                        "{treatment:?} light {light} alpha {alpha}"
                    );
                }
            }
        }
    }

    #[test]
    fn none_treatment_is_the_proxy() {
        let p = proxy(3, 3, |x, y| [x as u8, y as u8, 9, 255]);
        assert_eq!(treat(&p, ImageTreatment::None, true), p.bgra());
    }

    #[test]
    fn light_flag_is_normalized_for_none_and_dither() {
        let key = |t, a| TreatmentKey::new(t, a);
        use ImageTreatment::*;
        use TreatmentAppearance::*;
        assert_eq!(key(None, Dark), key(None, Light));
        assert_eq!(key(Dither, Dark), key(Dither, Light));
        for treatment in [Scanlines, Halftone, TextDensity] {
            assert_ne!(key(treatment, Dark), key(treatment, Light));
        }
        assert!(!key(Dither, Light).light() && key(Halftone, Light).light());
    }

    #[test]
    fn sniffing_reads_the_bytes() {
        use ImageFormat::*;
        assert_eq!(
            sniff_image_format(&[0x89, 0x50, 0x4E, 0x47, 0x0D]),
            Some(Png)
        );
        assert_eq!(sniff_image_format(&[0xFF, 0xD8, 0xFF]), Some(Jpeg));
        assert_eq!(sniff_image_format(b"GIF89a.."), Some(Gif));
        assert_eq!(sniff_image_format(b"GIF87a.."), Some(Gif));
        assert_eq!(sniff_image_format(b"RIFF\0\0\0\0WEBPVP8 "), Some(Webp));
        assert_eq!(sniff_image_format(b"RIFF\0\0\0\0WAVEfmt "), None);
        assert_eq!(sniff_image_format(b"BM\0\0"), Some(Bmp));
        assert_eq!(sniff_image_format(&[0x49, 0x49, 0x2A, 0x00]), Some(Tiff));
        assert_eq!(sniff_image_format(&[0x4D, 0x4D, 0x00, 0x2A]), Some(Tiff));
        assert_eq!(sniff_image_format(&[0, 0, 1, 0]), Some(Ico));
        assert_eq!(sniff_image_format(b"P6\n1 1"), Some(Pnm));
        assert_eq!(sniff_image_format(b"P7\n"), None);
        assert_eq!(sniff_image_format(b"  \n<svg xmlns=''/>"), Some(Svg));
        assert_eq!(sniff_image_format(b"<?xml version='1.0'?>"), Some(Svg));
        assert_eq!(sniff_image_format(b"hello"), None);
        assert_eq!(sniff_image_format(b""), None);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vespertine-treatment-{}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_png(path: &Path, width: u32, height: u32, shade: u8) {
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([shade, (x * 7) as u8, (y * 9) as u8, 255])
        });
        image
            .save_with_format(path, image::ImageFormat::Png)
            .unwrap();
    }

    #[vespertine::test]
    fn decoding_sniffs_the_bytes_and_survives_garbage(cx: &mut TestAppContext) {
        let dir = temp_dir("decode");
        // A PNG with a misleading extension decodes.
        let misleading = dir.join("actually-a-png.jpg");
        write_png(&misleading, 5, 3, 40);
        let renderer = cx.update(|cx| cx.svg_renderer());
        let decoded = decode_file(&misleading, &renderer).unwrap();
        assert_eq!(decoded.size(0).width.0, 5);
        assert_eq!(decoded.size(0).height.0, 3);
        // BGRA storage: pixel (0, 0) is red 40, green 0, blue 0.
        assert_eq!(&decoded.as_bytes(0).unwrap()[..4], &[0, 0, 40, 255]);

        // Undecodable and missing sources yield an error, never a panic.
        let garbage = dir.join("garbage.png");
        std::fs::write(&garbage, b"definitely not an image").unwrap();
        assert!(decode_file(&garbage, &renderer).is_err());
        let truncated = dir.join("truncated.png");
        std::fs::write(&truncated, &[0x89, 0x50, 0x4E, 0x47, 1, 2, 3]).unwrap();
        assert!(decode_file(&truncated, &renderer).is_err());
        assert!(decode_file(&dir.join("missing.png"), &renderer).is_err());

        // Through the cache: no image, no panic, and no endless retries.
        let cache = TreatmentCache::new(GlyphPalette::ink_ramp());
        for path in [&garbage, &truncated] {
            for _ in 0..3 {
                let image = cx.update(|cx| {
                    cache.request(
                        ImageTreatment::Scanlines,
                        path,
                        TreatmentAppearance::Dark,
                        cx,
                    )
                });
                assert!(image.is_none());
                cx.run_until_parked();
            }
        }
        assert_eq!(cache.jobs_started(), 2);
        assert!(cache.cached_sources().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[vespertine::test]
    fn requests_are_pending_then_ready_and_shared(cx: &mut TestAppContext) {
        let dir = temp_dir("cache");
        let path = dir.join("a.png");
        write_png(&path, 30, 20, 90);
        let cache = TreatmentCache::new(GlyphPalette::ink_ramp());
        let request = |cx: &mut TestAppContext, treatment, appearance| {
            cx.update(|cx| cache.request(treatment, &path, appearance, cx))
        };

        // Pending: nothing now, and repeated requests start no second job.
        assert!(request(cx, ImageTreatment::Scanlines, TreatmentAppearance::Dark).is_none());
        assert!(request(cx, ImageTreatment::Scanlines, TreatmentAppearance::Dark).is_none());
        assert_eq!(cache.jobs_started(), 1);
        cx.run_until_parked();

        // Ready: the same shared image object whatever the (irrelevant) viewport.
        let first = request(cx, ImageTreatment::Scanlines, TreatmentAppearance::Dark).unwrap();
        let second = request(cx, ImageTreatment::Scanlines, TreatmentAppearance::Dark).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(cache.jobs_started(), 1, "no regeneration");
        assert_eq!(first.size(0).width.0, 30);
        let proxy_before = cache.proxy(&path).unwrap();

        // Light splits scanlines but not none or dither.
        assert!(request(cx, ImageTreatment::Scanlines, TreatmentAppearance::Light).is_none());
        cx.run_until_parked();
        let light = request(cx, ImageTreatment::Scanlines, TreatmentAppearance::Light).unwrap();
        assert!(!Arc::ptr_eq(&first, &light));
        for treatment in [ImageTreatment::None, ImageTreatment::Dither] {
            assert!(request(cx, treatment, TreatmentAppearance::Dark).is_none());
            cx.run_until_parked();
            let dark = request(cx, treatment, TreatmentAppearance::Dark).unwrap();
            let jobs = cache.jobs_started();
            let light = request(cx, treatment, TreatmentAppearance::Light).unwrap();
            assert!(Arc::ptr_eq(&dark, &light));
            assert_eq!(cache.jobs_started(), jobs);
        }
        for treatment in [ImageTreatment::Halftone, ImageTreatment::TextDensity] {
            assert!(request(cx, treatment, TreatmentAppearance::Dark).is_none());
            cx.run_until_parked();
            assert!(request(cx, treatment, TreatmentAppearance::Dark).is_some());
        }
        // Every effect was generated once, from one shared proxy.
        assert_eq!(cache.jobs_started(), 6);
        assert!(Arc::ptr_eq(&proxy_before, &cache.proxy(&path).unwrap()));
        assert_eq!(cache.cached_sources(), std::slice::from_ref(&path));
        std::fs::remove_dir_all(dir).ok();
    }

    #[vespertine::test]
    fn the_cache_holds_four_sources(cx: &mut TestAppContext) {
        let dir = temp_dir("evict");
        let cache = TreatmentCache::new(GlyphPalette::ink_ramp());
        let paths: Vec<_> = (0..5)
            .map(|i| {
                let path = dir.join(format!("{i}.png"));
                write_png(&path, 8, 8, i as u8 * 10);
                path
            })
            .collect();
        for path in &paths {
            cx.update(|cx| {
                cache.request(ImageTreatment::None, path, TreatmentAppearance::Dark, cx)
            });
            cx.run_until_parked();
        }
        // The fifth source evicted the first.
        assert_eq!(cache.cached_sources(), paths[1..].to_vec());
        // Asking for the evicted source decodes it again, evicting the oldest.
        let jobs = cache.jobs_started();
        assert!(
            cx.update(|cx| cache.request(
                ImageTreatment::None,
                &paths[0],
                TreatmentAppearance::Dark,
                cx
            ))
            .is_none()
        );
        cx.run_until_parked();
        assert_eq!(cache.jobs_started(), jobs + 1);
        let mut expected = paths[2..].to_vec();
        expected.push(paths[0].clone());
        assert_eq!(cache.cached_sources(), expected);
        std::fs::remove_dir_all(dir).ok();
    }

    #[vespertine::test]
    fn preloaded_artwork_is_installed_without_a_job(cx: &mut TestAppContext) {
        let dir = temp_dir("preload");
        let path = dir.join("p.png");
        write_png(&path, 64, 64, 120);
        let cache = TreatmentCache::new(GlyphPalette::ink_ramp());
        let outcome: Rc<RefCell<Option<Vec<[u8; 4]>>>> = Rc::default();
        cx.update(|cx| {
            let task = cache.preload_file(
                &path,
                ImageTreatment::Halftone,
                TreatmentAppearance::Light,
                cx,
            );
            let outcome = outcome.clone();
            cx.spawn(async move |_| *outcome.borrow_mut() = Some(task.await.unwrap()))
                .detach();
        });
        cx.run_until_parked();
        let colors = outcome.borrow_mut().take().expect("preload finished");
        // 64 x 64 = 4096 pixels: stride 1.
        assert_eq!(colors.len(), 4096);
        assert_eq!(colors[0], [120, 0, 0, 255]);
        assert_eq!(cache.jobs_started(), 0);
        let image = cx
            .update(|cx| {
                cache.request(
                    ImageTreatment::Halftone,
                    &path,
                    TreatmentAppearance::Light,
                    cx,
                )
            })
            .expect("installed ready");
        assert_eq!(image.size(0).width.0, 64);
        assert_eq!(cache.jobs_started(), 0);

        // A decoded image is not decoded again; installing replaces the entry.
        let renderer = cx.update(|cx| cx.svg_renderer());
        let decoded = decode_file(&path, &renderer).unwrap();
        // The file is gone: preloading a decoded image must not decode it again.
        std::fs::remove_file(&path).unwrap();
        let outcome: Rc<RefCell<Option<Vec<[u8; 4]>>>> = Rc::default();
        cx.update(|cx| {
            let task = cache.preload_decoded(
                &path,
                decoded,
                ImageTreatment::Scanlines,
                TreatmentAppearance::Dark,
                cx,
            );
            let outcome = outcome.clone();
            cx.spawn(async move |_| *outcome.borrow_mut() = Some(task.await.unwrap()))
                .detach();
        });
        cx.run_until_parked();
        assert!(outcome.borrow().is_some());
        assert_eq!(cache.cached_sources(), std::slice::from_ref(&path));
        assert!(
            cx.update(|cx| cache.request(
                ImageTreatment::Halftone,
                &path,
                TreatmentAppearance::Light,
                cx
            ))
            .is_none(),
            "the replaced entry holds only the new effect"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    // Crossfade tracker.

    type Tracker = CrossfadeTracker<&'static str, &'static str>;

    fn image(shade: u8) -> Arc<RenderImage> {
        bgra_render_image(1, 1, vec![shade, shade, shade, 255]).unwrap()
    }

    fn ms(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn ids(frame: &CrossfadeFrame<&'static str>) -> (Option<usize>, Option<usize>) {
        (
            frame.current.as_ref().map(|(image, _)| image.id.0),
            frame.previous.as_ref().map(|(image, _)| image.id.0),
        )
    }

    #[test]
    fn tracker_holds_the_current_image_while_the_replacement_loads() {
        let (a, base) = (image(1), Instant::now());
        let mut tracker = Tracker::new();
        // Still loading with nothing shown: nothing, no blend.
        let frame = tracker.frame(None, &"a", &"f", true, false, base);
        assert_eq!(ids(&frame), (None, None));
        assert!(!frame.active && frame.mix == 1.0);
        // The first image blends in from nothing.
        let frame = tracker.frame(Some(&a), &"a", &"f", true, false, ms(base, 10));
        assert_eq!(ids(&frame), (Some(a.id.0), None));
        assert!(frame.active && frame.mix == 0.0);
        // After the blend, a replacement loading keeps the image visible.
        let frame = tracker.frame(None, &"a", &"f", true, false, ms(base, 400));
        assert_eq!(ids(&frame), (Some(a.id.0), None));
        assert!(!frame.active && frame.mix == 1.0);
        let frame = tracker.frame(None, &"a", &"f", true, false, ms(base, 500));
        assert_eq!(ids(&frame), (Some(a.id.0), None));
        assert!(!frame.active);
    }

    #[test]
    fn tracker_blends_over_180_ms_with_a_cubic_ease() {
        let (a, b, base) = (image(1), image(2), Instant::now());
        let mut tracker = Tracker::new();
        tracker.frame(Some(&a), &"a", &"fa", true, false, base);
        tracker.frame(None, &"a", &"fa", true, false, ms(base, 1000));

        let frame = tracker.frame(Some(&b), &"b", &"fb", true, false, ms(base, 2000));
        assert_eq!(ids(&frame), (Some(b.id.0), Some(a.id.0)));
        assert_eq!((frame.mix, frame.active), (0.0, true));
        assert_eq!(frame.current.as_ref().unwrap().1, "fb");
        assert_eq!(frame.previous.as_ref().unwrap().1, "fa");

        let frame = tracker.frame(Some(&b), &"b", &"fb", true, false, ms(base, 2045));
        assert!((frame.mix - (1.0 - (1.0f32 - 45.0 / 180.0).powi(3))).abs() < 1e-6);
        let frame = tracker.frame(Some(&b), &"b", &"fb", true, false, ms(base, 2090));
        assert!((frame.mix - 0.875).abs() < 1e-6, "{}", frame.mix);
        assert!(frame.active);
        let (previous, current) = frame.layer_opacities(0.8);
        assert!((previous - 0.1).abs() < 1e-6 && (current - 0.7).abs() < 1e-6);

        let frame = tracker.frame(Some(&b), &"b", &"fb", true, false, ms(base, 2180));
        assert_eq!((frame.mix, frame.active), (1.0, false));
        assert_eq!(ids(&frame), (Some(b.id.0), None));
    }

    #[test]
    fn tracker_skips_intermediate_images_and_restarts_only_after_the_blend() {
        let (a, b, c, d, base) = (image(1), image(2), image(3), image(4), Instant::now());
        let mut tracker = Tracker::new();
        tracker.frame(Some(&a), &"a", &"f", true, false, base);
        tracker.frame(None, &"a", &"f", true, false, ms(base, 1000));
        tracker.frame(Some(&b), &"b", &"f", true, false, ms(base, 2000));
        // c arrives mid-blend and waits; the blend is not restarted.
        let frame = tracker.frame(Some(&c), &"c", &"f", true, false, ms(base, 2060));
        assert_eq!(ids(&frame), (Some(b.id.0), Some(a.id.0)));
        assert!(frame.active && frame.mix > 0.0);
        // d replaces c before the blend ends: only the latest is adopted afterwards.
        let frame = tracker.frame(Some(&d), &"d", &"f", true, false, ms(base, 2170));
        assert_eq!(ids(&frame), (Some(b.id.0), Some(a.id.0)));
        // The blend completes and d is adopted in the same frame, starting at mix 0.
        let frame = tracker.frame(Some(&d), &"d", &"f", true, false, ms(base, 2180));
        assert_eq!(ids(&frame), (Some(d.id.0), Some(b.id.0)));
        assert_eq!((frame.mix, frame.active), (0.0, true));
        // c is never shown.
        let frame = tracker.frame(Some(&d), &"d", &"f", true, false, ms(base, 2360));
        assert_eq!(ids(&frame), (Some(d.id.0), None));
        assert!(!frame.active);
    }

    #[test]
    fn tracker_fades_out_when_disabled() {
        let (a, base) = (image(1), Instant::now());
        let mut tracker = Tracker::new();
        tracker.frame(Some(&a), &"a", &"fa", true, false, base);
        tracker.frame(None, &"a", &"fa", true, false, ms(base, 1000));
        let frame = tracker.frame(None, &"a", &"other", false, false, ms(base, 2000));
        assert_eq!(ids(&frame), (None, Some(a.id.0)));
        assert_eq!((frame.mix, frame.active), (0.0, true));
        assert_eq!(
            frame.previous.as_ref().unwrap().1,
            "fa",
            "the departing framing is kept"
        );
        let frame = tracker.frame(None, &"a", &"other", false, false, ms(base, 2180));
        assert_eq!(ids(&frame), (None, None));
        assert!(!frame.active);
        // Disabled with nothing shown stays idle.
        let frame = tracker.frame(None, &"a", &"other", false, false, ms(base, 3000));
        assert!(!frame.active && frame.current.is_none());
    }

    #[test]
    fn tracker_settles_immediately_with_reduced_motion() {
        let (a, b, base) = (image(1), image(2), Instant::now());
        let mut tracker = Tracker::new();
        let frame = tracker.frame(Some(&a), &"a", &"fa", true, true, base);
        assert_eq!(ids(&frame), (Some(a.id.0), None));
        assert_eq!((frame.mix, frame.active), (1.0, false));
        let frame = tracker.frame(Some(&b), &"b", &"fb", true, true, ms(base, 1));
        assert_eq!(ids(&frame), (Some(b.id.0), None));
        assert_eq!((frame.mix, frame.active), (1.0, false));
        let frame = tracker.frame(None, &"b", &"fb", false, true, ms(base, 2));
        assert_eq!(ids(&frame), (None, None));
        assert!(!frame.active);

        // Turning reduced motion on mid-blend ends the blend at once.
        let mut tracker = Tracker::new();
        tracker.frame(Some(&a), &"a", &"fa", true, false, base);
        tracker.frame(Some(&a), &"a", &"fa", true, false, ms(base, 500));
        tracker.frame(Some(&b), &"b", &"fb", true, false, ms(base, 1000));
        let frame = tracker.frame(Some(&b), &"b", &"fb", true, true, ms(base, 1010));
        assert_eq!(ids(&frame), (Some(b.id.0), None));
        assert!(!frame.active && frame.mix == 1.0);
    }

    #[test]
    fn tracker_framing_belongs_to_the_source() {
        let (a, a_dither, b, base) = (image(1), image(2), image(3), Instant::now());
        let mut tracker = Tracker::new();
        tracker.frame(Some(&a), &"a", &"crop1", true, false, base);
        tracker.frame(None, &"a", &"crop1", true, false, ms(base, 1000));

        // A framing change while the effect of the same source loads replaces the
        // framing immediately, without a fade.
        let frame = tracker.frame(None, &"a", &"crop2", true, false, ms(base, 1100));
        assert_eq!(frame.current.as_ref().unwrap().1, "crop2");
        assert!(!frame.active && frame.previous.is_none());
        assert_eq!(ids(&frame).0, Some(a.id.0));

        // The effect arrives: it fades in with the supplied framing, the departing
        // image keeps the framing it had.
        let frame = tracker.frame(Some(&a_dither), &"a", &"crop2", true, false, ms(base, 1200));
        assert_eq!(frame.current.as_ref().unwrap().1, "crop2");
        assert_eq!(frame.previous.as_ref().unwrap().1, "crop2");
        tracker.frame(Some(&a_dither), &"a", &"crop2", true, false, ms(base, 2000));

        // A replacement from another source carries its own framing and the
        // departing image keeps its own, whatever is supplied afterwards.
        let frame = tracker.frame(Some(&b), &"b", &"crop3", true, false, ms(base, 3000));
        assert_eq!(frame.current.as_ref().unwrap().1, "crop3");
        assert_eq!(frame.previous.as_ref().unwrap().1, "crop2");
        let frame = tracker.frame(Some(&b), &"b", &"crop4", true, false, ms(base, 3100));
        assert_eq!(frame.current.as_ref().unwrap().1, "crop4");
        assert_eq!(frame.previous.as_ref().unwrap().1, "crop2");

        // Removal keeps the departing framing.
        tracker.frame(Some(&b), &"b", &"crop4", true, false, ms(base, 4000));
        let frame = tracker.frame(None, &"b", &"crop5", false, false, ms(base, 5000));
        assert_eq!(frame.previous.as_ref().unwrap().1, "crop4");
    }
}
