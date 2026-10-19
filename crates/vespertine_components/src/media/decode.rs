//! Bounded raster and vector decoding (CMP-020 R10, R11, R12).
//!
//! Every entry point checks the input length first, decodes with hard limits (4096 px per side,
//! 64 MiB of decoder allocation), keeps only the first frame and reports every failure as a
//! [`MediaError`] value; none of them panics on hostile input. The functions are synchronous and
//! free of framework state so callers run them on a background executor.

use super::svg::{self, VectorSource};
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
    codecs::png::PngEncoder,
};
use std::fmt;
use std::io::Cursor;
use std::sync::Arc;
use vespertine::{Pixels, RenderImage, Size, SvgRenderer, px, size};

/// Byte cap for workspace previews.
pub const WORKSPACE_PREVIEW_BYTE_CAP: usize = 8 * 1024 * 1024;
/// Byte cap for ordinary attachments.
pub const ATTACHMENT_BYTE_CAP: usize = 24 * 1024 * 1024;
/// Largest side of a decoded raster.
pub const MAX_SIDE: u32 = 4096;
/// Largest decoder allocation.
pub const MAX_DECODER_ALLOC: u64 = 64 * 1024 * 1024;
/// Largest side of a generated preview.
pub const PREVIEW_MAX_SIDE: u32 = 2048;
/// Largest side of an icon-sized preview.
pub const ICON_MAX_SIDE: u32 = 64;
/// Accounted bytes per pixel (CPU pixels plus the GPU texture).
pub const BYTES_PER_PIXEL: usize = 8;

/// A recoverable decode failure; shown in the error state of the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaError {
    /// The input is larger than the caller's byte cap.
    TooLarge {
        /// Length of the input.
        bytes: usize,
        /// The cap that applied.
        cap: usize,
    },
    /// The declared MIME type is not supported.
    Unsupported,
    /// The sniffed format differs from the declared MIME type.
    FormatMismatch,
    /// The bytes are not a valid image (including truncated and over-limit input).
    Invalid(String),
    /// The vector document is invalid or exceeds a limit.
    Svg(String),
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaError::TooLarge { bytes, cap } => {
                write!(f, "image of {bytes} bytes exceeds the {cap} byte limit")
            }
            MediaError::Unsupported => f.write_str("unsupported"),
            MediaError::FormatMismatch => f.write_str("format mismatch"),
            MediaError::Invalid(message) => write!(f, "invalid image: {message}"),
            MediaError::Svg(message) => write!(f, "invalid SVG: {message}"),
        }
    }
}

impl std::error::Error for MediaError {}

/// Retained bytes of a decoded raster: its encoded bytes plus `width x height x 8` (CPU pixels
/// plus the GPU texture).
pub fn raster_retained_bytes(encoded_len: usize, width: u32, height: u32) -> usize {
    encoded_len + width as usize * height as usize * BYTES_PER_PIXEL
}

/// A decoded, displayable media item: the image, its natural size, the retained byte estimate
/// and, for vector media, the sanitized source and the current raster size.
#[derive(Clone)]
pub struct DecodedMedia {
    pub(crate) image: Arc<RenderImage>,
    pub(crate) natural: (f32, f32),
    pub(crate) retained_bytes: usize,
    pub(crate) encoded: Option<Arc<[u8]>>,
    pub(crate) vector: Option<VectorSource>,
}

impl fmt::Debug for DecodedMedia {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedMedia")
            .field("natural", &self.natural)
            .field("retained_bytes", &self.retained_bytes)
            .field("vector", &self.vector.is_some())
            .finish()
    }
}

impl DecodedMedia {
    /// The displayable image (the first frame only).
    pub fn image(&self) -> &Arc<RenderImage> {
        &self.image
    }

    /// Natural width and height in logical pixels. For vector media these are the declared
    /// logical dimensions, not the raster size.
    pub fn natural_size(&self) -> Size<Pixels> {
        size(px(self.natural.0), px(self.natural.1))
    }

    /// Natural width.
    pub fn natural_width(&self) -> f32 {
        self.natural.0
    }

    /// Natural height.
    pub fn natural_height(&self) -> f32 {
        self.natural.1
    }

    /// Estimated retained bytes: encoded bytes plus `width x height x 8` for a raster;
    /// `2 x source length + 1024 + raster width x raster height x 8` for a vector.
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    /// The re-encoded static PNG of a raster; `None` for vector media.
    pub fn encoded_png(&self) -> Option<&Arc<[u8]>> {
        self.encoded.as_ref()
    }

    /// Whether this is vector media.
    pub fn is_vector(&self) -> bool {
        self.vector.is_some()
    }

    /// The sanitized, re-serialized source of vector media.
    pub fn sanitized_source(&self) -> Option<&str> {
        self.vector.as_ref().map(|v| &*v.sanitized)
    }

    /// The raster size of vector media in device pixels.
    pub fn raster_size(&self) -> Option<(u32, u32)> {
        self.vector.as_ref().map(|v| v.raster)
    }

    /// Variant of this media for a view of `viewport` logical pixels at pixel ratio `dpr` with
    /// raster budget `pixel_budget` (R12). A variant whose raster size equals the current one
    /// is this media itself; raster media never changes.
    pub fn for_view(
        &self,
        viewport: Size<Pixels>,
        dpr: f32,
        pixel_budget: u64,
        renderer: &SvgRenderer,
    ) -> Result<DecodedMedia, MediaError> {
        let Some(vector) = &self.vector else {
            return Ok(self.clone());
        };
        let raster = svg::raster_size(
            self.natural,
            (f32::from(viewport.width), f32::from(viewport.height)),
            dpr,
            pixel_budget,
        );
        if raster == vector.raster {
            return Ok(self.clone());
        }
        svg::with_raster(self, vector, raster, renderer)
    }

    /// [`DecodedMedia::for_view`] with the preview viewport (900 x 480) and budget
    /// (1,048,576 pixels).
    pub fn preview_for_view(
        &self,
        dpr: f32,
        renderer: &SvgRenderer,
    ) -> Result<DecodedMedia, MediaError> {
        self.for_view(
            size(px(svg::PREVIEW_VIEWPORT.0), px(svg::PREVIEW_VIEWPORT.1)),
            dpr,
            svg::PREVIEW_PIXEL_BUDGET,
            renderer,
        )
    }

    /// Memory-aware re-raster ("preview within"): a larger raster for a new view is generated
    /// only when its estimated bytes do not exceed the current estimate or `available_bytes`,
    /// the memory the owner can still spend excluding this media; otherwise the current raster
    /// stays (slightly softer, never over budget).
    pub fn preview_within(
        &self,
        viewport: Size<Pixels>,
        dpr: f32,
        available_bytes: usize,
        renderer: &SvgRenderer,
    ) -> Result<DecodedMedia, MediaError> {
        let Some(vector) = &self.vector else {
            return Ok(self.clone());
        };
        let raster = svg::raster_size(
            self.natural,
            (f32::from(viewport.width), f32::from(viewport.height)),
            dpr,
            svg::PREVIEW_PIXEL_BUDGET,
        );
        if raster == vector.raster {
            return Ok(self.clone());
        }
        let estimate = svg::estimated_bytes(vector.sanitized.len(), raster);
        if estimate > self.retained_bytes && estimate > available_bytes {
            return Ok(self.clone());
        }
        svg::with_raster(self, vector, raster, renderer)
    }

    /// The enlarged variant for the full view with memory budget `budget_bytes` (default 32 MiB,
    /// see [`svg::DEFAULT_ENLARGED_BUDGET`]). `cached` is a previously derived variant that is
    /// reused when its raster size equals the result.
    pub fn enlarged(
        &self,
        dpr: f32,
        budget_bytes: usize,
        cached: Option<&DecodedMedia>,
        renderer: &SvgRenderer,
    ) -> Result<DecodedMedia, MediaError> {
        let Some(vector) = &self.vector else {
            return Ok(self.clone());
        };
        let Some(raster) = svg::enlarged_raster(
            self.natural,
            vector.sanitized.len(),
            vector.raster,
            dpr,
            budget_bytes,
        ) else {
            return Ok(self.clone());
        };
        if let Some(cached) = cached
            && cached.raster_size() == Some(raster)
        {
            return Ok(cached.clone());
        }
        if raster == vector.raster {
            return Ok(self.clone());
        }
        svg::with_raster(self, vector, raster, renderer)
    }
}

/// How a raster is limited and reduced.
#[derive(Clone, Copy)]
struct RasterPolicy {
    cap: usize,
    /// Reduce so neither side exceeds `min(max_side, larger source side)`.
    reduce_to: Option<u32>,
    /// Reduce to fit inside this box (list thumbnails: twice the display frame).
    fit_within: Option<(u32, u32)>,
}

fn mime_format(mime: &str) -> Option<ImageFormat> {
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/png" => Some(ImageFormat::Png),
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/webp" => Some(ImageFormat::WebP),
        "image/gif" => Some(ImageFormat::Gif),
        _ => None,
    }
}

fn is_svg_mime(mime: &str) -> bool {
    mime.trim().eq_ignore_ascii_case("image/svg+xml")
}

fn check_cap(bytes: &[u8], cap: usize) -> Result<(), MediaError> {
    if bytes.len() > cap {
        return Err(MediaError::TooLarge {
            bytes: bytes.len(),
            cap,
        });
    }
    Ok(())
}

fn invalid(error: impl fmt::Display) -> MediaError {
    MediaError::Invalid(error.to_string())
}

/// Decodes the first frame of `bytes` as a bounded raster and re-encodes it as a static PNG.
fn decode_raster(
    bytes: &[u8],
    policy: RasterPolicy,
    expected: Option<ImageFormat>,
) -> Result<DecodedMedia, MediaError> {
    check_cap(bytes, policy.cap)?;
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(invalid)?;
    let format = reader
        .format()
        .ok_or_else(|| invalid("unrecognized format"))?;
    if let Some(expected) = expected
        && expected != format
    {
        return Err(MediaError::FormatMismatch);
    }
    let mut decoder = reader.into_decoder().map_err(invalid)?;
    let (width, height) = decoder.dimensions();
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(invalid(format!(
            "{width} x {height} exceeds {MAX_SIDE} pixels per side"
        )));
    }
    if decoder.total_bytes() > MAX_DECODER_ALLOC {
        return Err(invalid(format!(
            "decoding would allocate {} bytes, above the {MAX_DECODER_ALLOC} byte limit",
            decoder.total_bytes()
        )));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODER_ALLOC);
    decoder.set_limits(limits).map_err(invalid)?;
    let orientation = decoder.orientation().map_err(invalid)?;
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(invalid)?;
    decoded.apply_orientation(orientation);
    if let Some(max_side) = policy.reduce_to {
        let bound = max_side.min(decoded.width().max(decoded.height()));
        if decoded.width() > bound || decoded.height() > bound {
            decoded = decoded.thumbnail(bound, bound);
        }
    }
    if let Some((bound_w, bound_h)) = policy.fit_within
        && (decoded.width() > bound_w || decoded.height() > bound_h)
    {
        decoded = decoded.thumbnail(bound_w, bound_h);
    }
    let rgba = decoded.into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(rgba.as_raw(), width, height, ExtendedColorType::Rgba8)
        .map_err(invalid)?;
    let mut bgra = rgba.into_raw();
    for pixel in bgra.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image =
        RenderImage::from_bgra(width, height, bgra).ok_or_else(|| invalid("empty image"))?;
    let retained_bytes = raster_retained_bytes(png.len(), width, height);
    Ok(DecodedMedia {
        image: Arc::new(image),
        natural: (width as f32, height as f32),
        retained_bytes,
        encoded: Some(png.into()),
        vector: None,
    })
}

/// Decodes `bytes` of declared type `mime` for an ordinary attachment (24 MiB cap): a raster
/// is decoded within the hard limits to its first frame; an SVG is sanitized and rasterized
/// for the preview viewport. Failures are error values.
pub fn decode_image(
    mime: &str,
    bytes: &[u8],
    renderer: &SvgRenderer,
) -> Result<DecodedMedia, MediaError> {
    decode_image_with_cap(mime, bytes, ATTACHMENT_BYTE_CAP, renderer)
}

/// [`decode_image`] with an explicit byte cap (8 MiB for workspace previews).
pub fn decode_image_with_cap(
    mime: &str,
    bytes: &[u8],
    cap: usize,
    renderer: &SvgRenderer,
) -> Result<DecodedMedia, MediaError> {
    if is_svg_mime(mime) {
        return svg::decode(bytes, cap, svg::PreviewKind::Full, renderer);
    }
    decode_raster(
        bytes,
        RasterPolicy {
            cap,
            reduce_to: None,
            fit_within: None,
        },
        None,
    )
}

/// Decodes a generated preview: the MIME type must be png, jpeg, webp or gif and the sniffed
/// format must equal it (otherwise "unsupported" / "format mismatch"); the image is reduced
/// with a thumbnail filter so neither side exceeds `min(2048, larger source side)` and
/// converted to 8-bit RGBA. `cap` is the caller's byte cap.
pub fn decode_generated_image(
    mime: &str,
    bytes: &[u8],
    cap: usize,
) -> Result<DecodedMedia, MediaError> {
    let expected = mime_format(mime).ok_or(MediaError::Unsupported)?;
    decode_raster(
        bytes,
        RasterPolicy {
            cap,
            reduce_to: Some(PREVIEW_MAX_SIDE),
            fit_within: None,
        },
        Some(expected),
    )
}

/// Decodes an icon-sized preview: like [`decode_generated_image`] with 64 px per side; an SVG
/// is rasterized for a 16 x 16 viewport with a 4096 pixel budget.
pub fn decode_icon(
    mime: &str,
    bytes: &[u8],
    cap: usize,
    renderer: &SvgRenderer,
) -> Result<DecodedMedia, MediaError> {
    if is_svg_mime(mime) {
        return svg::decode(bytes, cap, svg::PreviewKind::Icon, renderer);
    }
    let expected = mime_format(mime).ok_or(MediaError::Unsupported)?;
    decode_raster(
        bytes,
        RasterPolicy {
            cap,
            reduce_to: Some(ICON_MAX_SIDE),
            fit_within: None,
        },
        Some(expected),
    )
}

/// Decodes a list thumbnail: like [`decode_generated_image`], further reduced to fit within
/// twice the pixel size of its display `frame` (a 560 x 220 frame gives at most 1120 x 440).
pub fn decode_thumbnail(
    mime: &str,
    bytes: &[u8],
    cap: usize,
    frame: Size<Pixels>,
) -> Result<DecodedMedia, MediaError> {
    let expected = mime_format(mime).ok_or(MediaError::Unsupported)?;
    let bound = super::cache::thumbnail_bound(frame);
    let axis = |v: Pixels| (f32::from(v).round().max(1.0)) as u32;
    decode_raster(
        bytes,
        RasterPolicy {
            cap,
            reduce_to: Some(PREVIEW_MAX_SIDE),
            fit_within: Some((axis(bound.width), axis(bound.height))),
        },
        Some(expected),
    )
}
