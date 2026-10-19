//! Image source handling shared by the raster host and the live-update API (ELM-004):
//! source classification, data URLs, byte sniffing, pixel validation and bounded
//! decoding of untrusted bytes.

use crate::{
    ImageCacheError, ImageFormat, RenderImage, SvgRenderer, decode_static_image,
    decode_static_image_from_decoder,
};
use image::{
    AnimationDecoder, Frame, ImageDecoder as _, Limits, Rgba,
    codecs::{gif::GifDecoder, webp::WebPDecoder},
};
use smallvec::SmallVec;
use std::{io::Cursor, sync::Arc};
use thiserror::Error;

/// The classification of a raster host `src` string (ELM-004 R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageSrc<'a> {
    /// No source (empty after trimming).
    Empty,
    /// A `data:` URL (the whole trimmed text).
    DataUrl(&'a str),
    /// An `http` or `https` URI with a non-empty remainder after `://`.
    Network(&'a str),
    /// A local filesystem path (the trimmed text).
    Path(&'a str),
}

/// Classifies `src` after trimming surrounding whitespace.
///
/// A scheme match is case-insensitive. `http://` or `https://` with nothing after the
/// separator has no authority and is a local path.
pub fn classify_image_src(src: &str) -> ImageSrc<'_> {
    let src = src.trim();
    if src.is_empty() {
        return ImageSrc::Empty;
    }
    if starts_with_ignore_case(src, "data:") {
        return ImageSrc::DataUrl(src);
    }
    for scheme in ["http://", "https://"] {
        if starts_with_ignore_case(src, scheme) && src.len() > scheme.len() {
            return ImageSrc::Network(src);
        }
    }
    ImageSrc::Path(src)
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
        && text.is_char_boundary(prefix.len())
}

/// A decoded `data:` URL (ELM-004 R2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DataUrl {
    /// The lowercased MIME type.
    pub mime: String,
    /// The format the MIME type maps to. The bytes themselves are sniffed before decoding.
    pub format: ImageFormat,
    /// The decoded payload.
    pub bytes: Vec<u8>,
}

/// Why a data URL could not be used as an image source.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DataUrlError {
    /// The text does not start with `data:`.
    #[error("not a data URL")]
    NotDataUrl,
    /// There is no comma separating metadata from payload.
    #[error("malformed data URL: missing comma")]
    MissingComma,
    /// The MIME type is not a supported image type.
    #[error("unsupported data URL MIME type {0:?}")]
    UnsupportedMime(String),
    /// The base64 payload is not valid standard base64.
    #[error("invalid base64 payload in data URL")]
    InvalidBase64,
}

/// Maps a (lowercase) image MIME type to a supported format.
pub fn image_format_from_mime(mime: &str) -> Option<ImageFormat> {
    ImageFormat::from_mime_type(mime).or(match mime {
        "image/x-icon" | "image/vnd.microsoft.icon" => Some(ImageFormat::Ico),
        "image/x-portable-bitmap" | "image/x-portable-graymap" | "image/x-portable-pixmap" => {
            Some(ImageFormat::Pnm)
        }
        _ => None,
    })
}

/// Parses a `data:` URL: MIME detection, base64 and percent decoding.
///
/// The text after `data:` up to the first comma is the metadata; its first
/// semicolon-separated part is the MIME type (lowercased). If any later part equals
/// `base64` (case-insensitive) the payload is standard base64, otherwise it is
/// percent-decoded (a `%` not followed by two hex digits is kept literally).
pub fn parse_data_url(text: &str) -> Result<DataUrl, DataUrlError> {
    let text = text.trim();
    if !starts_with_ignore_case(text, "data:") {
        return Err(DataUrlError::NotDataUrl);
    }
    let rest = &text["data:".len()..];
    let (metadata, payload) = rest.split_once(',').ok_or(DataUrlError::MissingComma)?;
    let mut parts = metadata.split(';');
    let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let format =
        image_format_from_mime(&mime).ok_or_else(|| DataUrlError::UnsupportedMime(mime.clone()))?;
    let is_base64 = parts.any(|part| part.trim().eq_ignore_ascii_case("base64"));
    let bytes = if is_base64 {
        decode_base64(payload).ok_or(DataUrlError::InvalidBase64)?
    } else {
        percent_decode(payload)
    };
    Ok(DataUrl {
        mime,
        format,
        bytes,
    })
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Standard-alphabet base64 with required padding.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn value(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a') as u32 + 26),
            b'0'..=b'9' => Some((byte - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let chunk_count = bytes.len() / 4;
    for (chunk_index, chunk) in bytes.chunks_exact(4).enumerate() {
        let is_last = chunk_index + 1 == chunk_count;
        let padding = chunk.iter().rev().take_while(|byte| **byte == b'=').count();
        if padding > 2 || (padding > 0 && !is_last) {
            return None;
        }
        let mut accumulator = 0u32;
        for (position, byte) in chunk.iter().enumerate() {
            let digit = if position >= 4 - padding {
                0
            } else {
                value(*byte)?
            };
            accumulator = accumulator << 6 | digit;
        }
        let triple = accumulator.to_be_bytes();
        out.extend_from_slice(&triple[1..4 - padding]);
    }
    Some(out)
}

/// Detects the image format from the actual bytes (ELM-004 R11), in order: PNG, JPEG, GIF,
/// WebP, BMP, TIFF, ICO, PNM, SVG. Returns `None` when nothing matches.
pub fn sniff_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
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
        let trimmed = bytes
            .iter()
            .position(|byte| !byte.is_ascii_whitespace())
            .map(|start| &bytes[start..])
            .unwrap_or(&[]);
        if trimmed.starts_with(b"<svg") || trimmed.starts_with(b"<?xml") {
            Some(ImageFormat::Svg)
        } else {
            None
        }
    }
}

fn to_image_crate_format(format: ImageFormat) -> Option<image::ImageFormat> {
    match format {
        ImageFormat::Png => Some(image::ImageFormat::Png),
        ImageFormat::Jpeg => Some(image::ImageFormat::Jpeg),
        ImageFormat::Webp => Some(image::ImageFormat::WebP),
        ImageFormat::Gif => Some(image::ImageFormat::Gif),
        ImageFormat::Bmp => Some(image::ImageFormat::Bmp),
        ImageFormat::Tiff => Some(image::ImageFormat::Tiff),
        ImageFormat::Ico => Some(image::ImageFormat::Ico),
        ImageFormat::Pnm => Some(image::ImageFormat::Pnm),
        ImageFormat::Svg => None,
    }
}

pub(crate) fn unrecognized_image_format() -> ImageCacheError {
    ImageCacheError::Other(Arc::new(anyhow::anyhow!("unrecognized image format")))
}

/// Decodes image bytes by content (never by extension or declared type).
///
/// Animated GIF and WebP keep all frames; SVG is rasterized by `renderer` at twice its
/// declared size (the color-preserving path of ELM-004 R17). Unrecognized bytes fail with
/// "unrecognized image format".
pub fn decode_image_bytes(
    bytes: &[u8],
    renderer: &SvgRenderer,
) -> Result<Arc<RenderImage>, ImageCacheError> {
    let format = sniff_image_format(bytes).ok_or_else(unrecognized_image_format)?;
    decode_sniffed(bytes, format, renderer)
}

pub(crate) fn decode_sniffed(
    bytes: &[u8],
    format: ImageFormat,
    renderer: &SvgRenderer,
) -> Result<Arc<RenderImage>, ImageCacheError> {
    let frames = match format {
        ImageFormat::Svg => return Ok(renderer.render_single_frame(bytes, 1.0)?),
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(bytes))?;
            collect_frames(decoder.into_frames(), "GIF")?
        }
        ImageFormat::Webp => {
            let mut decoder = WebPDecoder::new(Cursor::new(bytes))?;
            if decoder.has_animation() {
                let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
                collect_frames(decoder.into_frames(), "WebP")?
            } else {
                decode_static_image_from_decoder(decoder)?
            }
        }
        other => decode_static_image(
            bytes,
            to_image_crate_format(other).expect("every non-SVG format has a decoder"),
        )?,
    };
    Ok(Arc::new(RenderImage::new(frames)))
}

fn collect_frames(
    frames: image::Frames<'_>,
    label: &str,
) -> Result<SmallVec<[Frame; 1]>, ImageCacheError> {
    let mut collected = SmallVec::new();
    for frame in frames {
        match frame {
            Ok(mut frame) => {
                // Convert from RGBA to BGRA.
                for pixel in frame.buffer_mut().chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                collected.push(frame);
            }
            Err(error) => log::debug!("Skipping {label} frame due to decode error: {error}"),
        }
    }
    if collected.is_empty() {
        return Err(ImageCacheError::Other(Arc::new(anyhow::anyhow!(
            "{label} could not be decoded: all frames failed"
        ))));
    }
    Ok(collected)
}

/// Limits applied before untrusted bytes reach a decoder (ELM-004 R28).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UntrustedImageLimits {
    /// Maximum input length in bytes, checked before any decoding.
    pub max_input_bytes: usize,
    /// Maximum raster width and height in pixels.
    pub max_side: u32,
    /// Maximum decoder allocation in bytes.
    pub max_decoder_alloc: u64,
}

impl Default for UntrustedImageLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 16 * 1024 * 1024,
            max_side: 4096,
            max_decoder_alloc: 64 * 1024 * 1024,
        }
    }
}

/// Decodes untrusted bytes with bounded cost: the input length is checked first; rasters are
/// decoded with `limits` and keep a single static frame (animated formats yield their first
/// frame); SVG is first rebuilt by [`SvgRenderer::prepare_untrusted_svg`] so scripts, HTML and
/// external references never reach the renderer.
pub fn decode_untrusted_image_bytes(
    bytes: &[u8],
    renderer: &SvgRenderer,
    limits: &UntrustedImageLimits,
) -> Result<Arc<RenderImage>, ImageCacheError> {
    if bytes.len() > limits.max_input_bytes {
        return Err(ImageCacheError::Other(Arc::new(anyhow::anyhow!(
            "image of {} bytes exceeds the {} byte limit",
            bytes.len(),
            limits.max_input_bytes
        ))));
    }
    let format = sniff_image_format(bytes).ok_or_else(unrecognized_image_format)?;
    if format == ImageFormat::Svg {
        let prepared = renderer.prepare_untrusted_svg(bytes, limits)?;
        return Ok(renderer.render_single_frame(prepared.as_bytes(), 1.0)?);
    }
    let format = to_image_crate_format(format).expect("raster formats have decoders");
    let mut decoder = image::ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .map_err(ImageCacheError::from)?;
    if decoder.total_bytes() > limits.max_decoder_alloc {
        return Err(ImageCacheError::Other(Arc::new(anyhow::anyhow!(
            "decoding would allocate {} bytes, above the {} byte limit",
            decoder.total_bytes(),
            limits.max_decoder_alloc
        ))));
    }
    let mut decoder_limits = Limits::default();
    decoder_limits.max_image_width = Some(limits.max_side);
    decoder_limits.max_image_height = Some(limits.max_side);
    decoder_limits.max_alloc = Some(limits.max_decoder_alloc);
    decoder
        .set_limits(decoder_limits)
        .map_err(ImageCacheError::from)?;
    let orientation = decoder.orientation().map_err(ImageCacheError::from)?;
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(ImageCacheError::from)?;
    image.apply_orientation(orientation);
    let mut data = image.into_rgba8();
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(
        SmallVec::<[Frame; 1]>::from_elem(Frame::new(data), 1),
    )))
}

/// The pixel layout of a packed straight-alpha buffer (ELM-004 R8).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PixelFormat {
    /// Red, green, blue, alpha (the default).
    #[default]
    Rgba,
    /// Blue, green, red, alpha: the stored order of decoded images.
    Bgra,
}

impl PixelFormat {
    /// The format name accepted at the host boundary.
    pub fn name(self) -> &'static str {
        match self {
            PixelFormat::Rgba => "rgba",
            PixelFormat::Bgra => "bgra",
        }
    }

    /// Parses a format name; absent means `rgba`. Only `rgba` and `bgra` are accepted.
    pub fn parse(name: Option<&str>) -> Result<Self, ImageUpdateError> {
        match name {
            None | Some("rgba") => Ok(PixelFormat::Rgba),
            Some("bgra") => Ok(PixelFormat::Bgra),
            Some(other) => Err(ImageUpdateError::UnsupportedFormat(other.to_string())),
        }
    }
}

/// An error from a live image update.
#[derive(Clone, Debug, Error, PartialEq)]
pub enum ImageUpdateError {
    /// A dimension was not a positive integer fitting in 32 bits.
    #[error("{name} must be a positive integer that fits in 32 bits, got {value}")]
    InvalidDimension {
        /// Which dimension (`width` or `height`).
        name: &'static str,
        /// The rejected value.
        value: f64,
    },
    /// The pixel format name is not `rgba` or `bgra`.
    #[error("unsupported pixel format {0:?}: expected \"rgba\" or \"bgra\"")]
    UnsupportedFormat(String),
    /// `width * height * 4` does not fit in the address space.
    #[error("{format} pixel buffer size {width} x {height} x 4 overflows")]
    SizeOverflow {
        /// Format name.
        format: &'static str,
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// The buffer length is not exactly `width * height * 4`.
    #[error(
        "{format} pixel buffer has {actual} bytes but {width} x {height} x 4 = {expected} bytes are required"
    )]
    LengthMismatch {
        /// Format name.
        format: &'static str,
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
        /// Required length.
        expected: usize,
        /// Provided length.
        actual: usize,
    },
    /// Encoded bytes were not recognized or could not be decoded.
    #[error("{0}")]
    Decode(String),
}

/// Validates a host-supplied dimension: a positive integer that fits in an unsigned 32-bit
/// value. Zero, negative, fractional, infinite, NaN and too-large values are rejected.
pub fn validate_image_dimension(name: &'static str, value: f64) -> Result<u32, ImageUpdateError> {
    if value.is_finite() && value.fract() == 0.0 && value >= 1.0 && value <= u32::MAX as f64 {
        Ok(value as u32)
    } else {
        Err(ImageUpdateError::InvalidDimension { name, value })
    }
}

/// Builds a single-frame image from tightly packed straight-alpha pixels.
///
/// The length must equal `width * height * 4` (overflow-checked). BGRA bytes are stored
/// unchanged; RGBA bytes exchange red and blue to reach the stored order.
pub fn render_image_from_pixels(
    width: f64,
    height: f64,
    bytes: &[u8],
    format: PixelFormat,
) -> Result<RenderImage, ImageUpdateError> {
    let width = validate_image_dimension("width", width)?;
    let height = validate_image_dimension("height", height)?;
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(ImageUpdateError::SizeOverflow {
            format: format.name(),
            width,
            height,
        })?;
    if bytes.len() != expected {
        return Err(ImageUpdateError::LengthMismatch {
            format: format.name(),
            width,
            height,
            expected,
            actual: bytes.len(),
        });
    }
    let mut stored = bytes.to_vec();
    if format == PixelFormat::Rgba {
        for pixel in stored.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
    }
    Ok(bgra_render_image(width, height, stored))
}

/// Raw single-frame BGRA construction (straight alpha, no swizzle) through the raw
/// constructor of RND-007; callers have validated the length.
pub(crate) fn bgra_render_image(width: u32, height: u32, bytes: Vec<u8>) -> RenderImage {
    RenderImage::from_bgra(width, height, bytes)
        .expect("caller validated the buffer length against width x height x 4")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat as CodecFormat};

    fn renderer() -> SvgRenderer {
        SvgRenderer::new(Arc::new(()))
    }

    fn encode(format: CodecFormat, width: u32, height: u32) -> Vec<u8> {
        let buffer = ImageBuffer::from_fn(width, height, |x, y| {
            Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        });
        let image = image::DynamicImage::ImageRgba8(buffer);
        let image = if format == CodecFormat::Jpeg {
            image::DynamicImage::ImageRgb8(image.to_rgb8())
        } else {
            image
        };
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, format).expect("encode");
        out.into_inner()
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let value = (chunk[0] as u32) << 16
                | (*chunk.get(1).unwrap_or(&0) as u32) << 8
                | *chunk.get(2).unwrap_or(&0) as u32;
            for index in 0..4 {
                if index <= chunk.len() {
                    out.push(ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn percent_encode(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("%{byte:02X}")).collect()
    }

    #[test]
    fn classifies_sources() {
        assert_eq!(classify_image_src("   "), ImageSrc::Empty);
        assert_eq!(classify_image_src(""), ImageSrc::Empty);
        assert_eq!(
            classify_image_src(" Data:image/png;base64,AA== "),
            ImageSrc::DataUrl("Data:image/png;base64,AA==")
        );
        assert_eq!(
            classify_image_src("HTTPS://a.b/x.png"),
            ImageSrc::Network("HTTPS://a.b/x.png")
        );
        assert_eq!(
            classify_image_src("http://a"),
            ImageSrc::Network("http://a")
        );
        assert_eq!(classify_image_src("http://"), ImageSrc::Path("http://"));
        assert_eq!(classify_image_src("https://"), ImageSrc::Path("https://"));
        assert_eq!(classify_image_src("ftp://x/y"), ImageSrc::Path("ftp://x/y"));
        assert_eq!(
            classify_image_src("/tmp/a.png"),
            ImageSrc::Path("/tmp/a.png")
        );
    }

    #[test]
    fn data_urls_decode_base64_and_percent_with_mixed_case() {
        let png = encode(CodecFormat::Png, 3, 2);
        let base64 = format!("DaTa:Image/PNG;BaSe64,{}", base64_encode(&png));
        let parsed = parse_data_url(&base64).expect("base64 data url");
        assert_eq!(parsed.mime, "image/png");
        assert_eq!(parsed.format, ImageFormat::Png);
        assert_eq!(parsed.bytes, png);

        let percent = format!("data:IMAGE/PNG,{}", percent_encode(&png));
        let parsed = parse_data_url(&percent).expect("percent data url");
        assert_eq!(parsed.bytes, png);

        // Padding variants and parameters before base64.
        let parsed = parse_data_url("data:image/svg+xml;charset=utf-8;base64,PHN2Zz4=").unwrap();
        assert_eq!(parsed.format, ImageFormat::Svg);
        assert_eq!(parsed.bytes, b"<svg>");

        // A percent sign without two hex digits is literal.
        let parsed = parse_data_url("data:image/svg+xml,<svg>%zz%4").unwrap();
        assert_eq!(parsed.bytes, b"<svg>%zz%4");
        let parsed = parse_data_url("data:image/svg+xml,a%20b%3Cc").unwrap();
        assert_eq!(parsed.bytes, b"a b<c");
    }

    #[test]
    fn malformed_data_urls_are_rejected() {
        assert_eq!(
            parse_data_url("data:image/png"),
            Err(DataUrlError::MissingComma)
        );
        assert_eq!(
            parse_data_url("data:text/plain,hi"),
            Err(DataUrlError::UnsupportedMime("text/plain".into()))
        );
        assert_eq!(
            parse_data_url("data:,hi"),
            Err(DataUrlError::UnsupportedMime(String::new()))
        );
        assert_eq!(
            parse_data_url("data:image/png;base64,@@@@"),
            Err(DataUrlError::InvalidBase64)
        );
        assert_eq!(
            parse_data_url("data:image/png;base64,QUJD="),
            Err(DataUrlError::InvalidBase64)
        );
        assert_eq!(
            parse_data_url("image/png,aa"),
            Err(DataUrlError::NotDataUrl)
        );
    }

    #[test]
    fn sniffing_follows_the_magic_byte_table() {
        let cases: [(&[u8], Option<ImageFormat>); 20] = [
            (
                &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A],
                Some(ImageFormat::Png),
            ),
            (&[0xFF, 0xD8, 0xFF, 0xE0], Some(ImageFormat::Jpeg)),
            (b"GIF87a....", Some(ImageFormat::Gif)),
            (b"GIF89a....", Some(ImageFormat::Gif)),
            (b"RIFF\x00\x00\x00\x00WEBPVP8 ", Some(ImageFormat::Webp)),
            (b"RIFF\x00\x00\x00\x00WAVEfmt ", None),
            (b"BM\x00\x00", Some(ImageFormat::Bmp)),
            (&[0x49, 0x49, 0x2A, 0x00], Some(ImageFormat::Tiff)),
            (&[0x4D, 0x4D, 0x00, 0x2A], Some(ImageFormat::Tiff)),
            (&[0x00, 0x00, 0x01, 0x00, 0x01], Some(ImageFormat::Ico)),
            (b"P1\n1 1\n0", Some(ImageFormat::Pnm)),
            (b"P6\n1 1\n255\n", Some(ImageFormat::Pnm)),
            (b"P7\n", None),
            (b"  \n<svg xmlns=''/>", Some(ImageFormat::Svg)),
            (b"<?xml version='1.0'?><svg/>", Some(ImageFormat::Svg)),
            (b"<html><svg/></html>", None),
            (b"GIF8", None),
            (b"", None),
            (b"hello world", None),
            (&[0x00, 0x00, 0x02, 0x00], None),
        ];
        for (bytes, expected) in cases {
            assert_eq!(sniff_image_format(bytes), expected, "{bytes:?}");
        }
        // Real encodings are detected by content.
        for (format, expected) in [
            (CodecFormat::Png, ImageFormat::Png),
            (CodecFormat::Jpeg, ImageFormat::Jpeg),
            (CodecFormat::Gif, ImageFormat::Gif),
            (CodecFormat::Bmp, ImageFormat::Bmp),
            (CodecFormat::Tiff, ImageFormat::Tiff),
            (CodecFormat::Ico, ImageFormat::Ico),
        ] {
            let bytes = encode(format, 4, 4);
            assert_eq!(sniff_image_format(&bytes), Some(expected), "{format:?}");
            let decoded = decode_image_bytes(&bytes, &renderer()).expect("decode by content");
            assert_eq!(decoded.size(0).width.0, 4, "{format:?}");
        }
    }

    #[test]
    fn unknown_bytes_are_unrecognized() {
        let error = decode_image_bytes(b"definitely not an image", &renderer()).unwrap_err();
        assert!(error.to_string().contains("unrecognized image format"));
    }

    #[test]
    fn pixel_dimension_validation() {
        for (name, value) in [
            ("zero", 0.0),
            ("negative", -3.0),
            ("fractional", 1.5),
            ("infinite", f64::INFINITY),
            ("nan", f64::NAN),
            ("too large", 4_294_967_296.0),
        ] {
            let error = validate_image_dimension("width", value).unwrap_err();
            assert!(
                matches!(
                    error,
                    ImageUpdateError::InvalidDimension { name: "width", .. }
                ),
                "{name}"
            );
            assert!(error.to_string().contains("width"), "{name}");
        }
        assert_eq!(validate_image_dimension("height", 1.0), Ok(1));
        assert_eq!(
            validate_image_dimension("height", u32::MAX as f64),
            Ok(u32::MAX)
        );
    }

    #[test]
    fn pixel_buffers_are_validated() {
        let wrong = render_image_from_pixels(2.0, 2.0, &[0; 15], PixelFormat::Rgba).unwrap_err();
        assert_eq!(
            wrong,
            ImageUpdateError::LengthMismatch {
                format: "rgba",
                width: 2,
                height: 2,
                expected: 16,
                actual: 15
            }
        );
        let message = render_image_from_pixels(2.0, 2.0, &[0; 17], PixelFormat::Bgra)
            .unwrap_err()
            .to_string();
        assert!(message.contains("bgra") && message.contains("17") && message.contains("16"));

        let overflow =
            render_image_from_pixels(u32::MAX as f64, u32::MAX as f64, &[0; 4], PixelFormat::Rgba)
                .unwrap_err();
        assert!(matches!(
            overflow,
            ImageUpdateError::SizeOverflow { format: "rgba", .. }
        ));

        assert!(render_image_from_pixels(0.0, 2.0, &[], PixelFormat::Rgba).is_err());
        assert!(render_image_from_pixels(-1.0, 2.0, &[0; 8], PixelFormat::Rgba).is_err());
        assert!(render_image_from_pixels(1.5, 2.0, &[0; 12], PixelFormat::Rgba).is_err());
        assert_eq!(PixelFormat::parse(None), Ok(PixelFormat::Rgba));
        assert_eq!(PixelFormat::parse(Some("rgba")), Ok(PixelFormat::Rgba));
        assert_eq!(PixelFormat::parse(Some("bgra")), Ok(PixelFormat::Bgra));
        for bad in ["argb", "RGBA", "", "rgb"] {
            let error = PixelFormat::parse(Some(bad)).unwrap_err();
            assert!(
                matches!(error, ImageUpdateError::UnsupportedFormat(_)),
                "{bad}"
            );
        }
    }

    #[test]
    fn rgba_and_bgra_store_the_same_colors_with_straight_alpha() {
        // Red, then a 50% alpha green, in both layouts.
        let rgba = render_image_from_pixels(
            2.0,
            1.0,
            &[255, 0, 0, 255, 0, 255, 0, 128],
            PixelFormat::Rgba,
        )
        .unwrap();
        let bgra = render_image_from_pixels(
            2.0,
            1.0,
            &[0, 0, 255, 255, 0, 255, 0, 128],
            PixelFormat::Bgra,
        )
        .unwrap();
        assert_eq!(rgba.as_bytes(0), bgra.as_bytes(0));
        // Stored order is blue, green, red, alpha; alpha is not premultiplied.
        assert_eq!(bgra.as_bytes(0).unwrap(), &[0, 0, 255, 255, 0, 255, 0, 128]);
        assert_eq!(rgba.frame_count(), 1);
        assert_ne!(rgba.id, bgra.id);
    }

    #[test]
    fn untrusted_raster_decoding_is_bounded() {
        let limits = UntrustedImageLimits::default();
        assert_eq!(limits.max_side, 4096);
        assert_eq!(limits.max_decoder_alloc, 64 * 1024 * 1024);

        let ok = encode(CodecFormat::Png, 4096, 1);
        let decoded = decode_untrusted_image_bytes(&ok, &renderer(), &limits).expect("4096 wide");
        assert_eq!(decoded.size(0).width.0, 4096);

        let too_wide = encode(CodecFormat::Png, 4097, 1);
        assert!(decode_untrusted_image_bytes(&too_wide, &renderer(), &limits).is_err());
        let too_tall = encode(CodecFormat::Png, 1, 4097);
        assert!(decode_untrusted_image_bytes(&too_tall, &renderer(), &limits).is_err());

        // The allocation bound applies independently of the side bound.
        let small_alloc = UntrustedImageLimits {
            max_decoder_alloc: 1024,
            ..limits
        };
        let image = encode(CodecFormat::Png, 64, 64);
        assert!(decode_untrusted_image_bytes(&image, &renderer(), &small_alloc).is_err());
        assert!(decode_untrusted_image_bytes(&image, &renderer(), &limits).is_ok());

        // The input length is checked before any decoding.
        let short_input = UntrustedImageLimits {
            max_input_bytes: 8,
            ..limits
        };
        let error = decode_untrusted_image_bytes(&image, &renderer(), &short_input).unwrap_err();
        assert!(error.to_string().contains("exceeds"));
    }

    #[test]
    fn untrusted_animated_images_keep_one_static_frame() {
        let mut out = Cursor::new(Vec::new());
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            for shade in [0u8, 255] {
                let frame =
                    image::Frame::new(ImageBuffer::from_pixel(2, 2, Rgba([shade, 0, 0, 255])));
                encoder.encode_frame(frame).expect("gif frame");
            }
        }
        let bytes = out.into_inner();
        let animated = decode_image_bytes(&bytes, &renderer()).unwrap();
        assert_eq!(animated.frame_count(), 2);
        let preview =
            decode_untrusted_image_bytes(&bytes, &renderer(), &UntrustedImageLimits::default())
                .unwrap();
        assert_eq!(preview.frame_count(), 1);
    }

    #[test]
    fn untrusted_svg_is_rebuilt_without_scripts_html_or_external_references() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"
                 width="80" height="40" viewBox="0 0 80 40">
            <script>alert(1)</script>
            <foreignObject width="80" height="40"><div xmlns="http://www.w3.org/1999/xhtml">html</div></foreignObject>
            <image href="data:image/png;base64,iVBORw0KGgo=" width="10" height="10"/>
            <image xlink:href="http://example.com/secret.png" width="10" height="10"/>
            <image xlink:href="/etc/hostname" width="10" height="10"/>
            <rect width="40" height="40" fill="#ff0000" onclick="alert(2)"/>
            <text x="2" y="30" font-family="NoSuchFontFamily123" font-size="20">Hi</text>
        </svg>"##;
        let prepared = renderer()
            .prepare_untrusted_svg(svg.as_bytes(), &UntrustedImageLimits::default())
            .expect("prepared");
        for forbidden in [
            "script",
            "foreignObject",
            "<image",
            "href",
            "example.com",
            "secret.png",
            "etc/hostname",
            "onclick",
            "alert",
        ] {
            assert!(
                !prepared.contains(forbidden),
                "{forbidden} survived: {prepared}"
            );
        }
        assert!(
            prepared.contains("<rect") || prepared.contains("<path"),
            "{prepared}"
        );
        // Text survives with a fallback font although the requested family is missing: a
        // document consisting only of that text still has painted content.
        let text_only = r#"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="40">
            <text x="2" y="30" font-family="NoSuchFontFamily123" font-size="20">Hi</text></svg>"#;
        let prepared_text = renderer()
            .prepare_untrusted_svg(text_only.as_bytes(), &UntrustedImageLimits::default())
            .expect("prepared text");
        assert!(
            prepared_text.contains("<path"),
            "text vanished: {prepared_text}"
        );
        // The prepared document renders.
        let rendered = renderer()
            .render_single_frame(prepared.as_bytes(), 1.0)
            .unwrap();
        assert_eq!(rendered.size(0).width.0, 160);
    }

    #[test]
    fn untrusted_svg_size_is_bounded() {
        let huge = br#"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="10"/>"#;
        let limits = UntrustedImageLimits::default();
        assert!(renderer().prepare_untrusted_svg(huge, &limits).is_err());
        let small = UntrustedImageLimits {
            max_input_bytes: 10,
            ..limits
        };
        assert!(renderer().prepare_untrusted_svg(huge, &small).is_err());
    }
}
