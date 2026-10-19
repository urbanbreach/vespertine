//! Vector media: sanitizing, declared size and raster sizing (CMP-020 R11, R12).
//!
//! An untrusted SVG is parsed with external resource loading disabled and re-serialized by the
//! framework renderer (`SvgRenderer::prepare_untrusted_svg`), so scripts, HTML and external
//! references never reach the rasterizer. The raster shown is made by wrapping the sanitized
//! document in an outer SVG whose declared size is half the raster size and whose view box is
//! the natural size, because the framework rasterizes vector images at twice the declared size.

use super::decode::{DecodedMedia, MediaError};
use std::sync::Arc;
use vespertine::{SvgRenderer, UntrustedImageLimits};

/// Default viewport of a preview.
pub const PREVIEW_VIEWPORT: (f32, f32) = (900.0, 480.0);
/// Default pixel budget of a preview raster.
pub const PREVIEW_PIXEL_BUDGET: u64 = 1_048_576;
/// Viewport of an icon-sized preview.
pub const ICON_VIEWPORT: (f32, f32) = (16.0, 16.0);
/// Pixel budget of an icon-sized preview raster.
pub const ICON_PIXEL_BUDGET: u64 = 4096;
/// Reference viewport of the full view.
pub const FULL_VIEW_VIEWPORT: (f32, f32) = (1600.0, 1000.0);
/// Default memory budget of the enlarged variant.
pub const DEFAULT_ENLARGED_BUDGET: usize = 32 * 1024 * 1024;
/// Pixel cap of the enlarged variant.
pub const ENLARGED_PIXEL_CAP: u64 = 2 * 1_048_576;
/// Largest side of any vector raster.
pub const MAX_RASTER_SIDE: f32 = 4096.0;
/// Device pixel ratio used for the first raster of a decoded vector.
pub const DEFAULT_DPR: f32 = 2.0;
/// Largest declared logical side accepted (bounds layout coordinates).
pub const MAX_DECLARED_SIDE: u32 = 131_072;

/// The sanitized source and the current raster size of vector media.
#[derive(Clone, Debug)]
pub struct VectorSource {
    pub(crate) sanitized: Arc<str>,
    pub(crate) raster: (u32, u32),
}

/// The kind of first raster to produce when decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    /// Preview viewport 900 x 480 at ratio 2 with 1,048,576 pixels.
    Full,
    /// Icon viewport 16 x 16 at ratio 2 with 4096 pixels.
    Icon,
}

/// The raster size for a natural size, a target viewport, a device pixel ratio (clamped to
/// `[1, 4]`) and a pixel budget (R12).
///
/// `fit = min(vx / w, vy / h, 1)`; `scale = min(fit x d, 4096 / w, 4096 / h, sqrt(P / (w h)))`;
/// the size is `(max(1, floor(w scale)), max(1, floor(h scale)))`; if the product still exceeds
/// `P` the longer side (width if width > height, else height) is reduced to
/// `max(1, P / other side)`.
pub fn raster_size(
    natural: (f32, f32),
    viewport: (f32, f32),
    dpr: f32,
    pixel_budget: u64,
) -> (u32, u32) {
    let (w, h) = (natural.0 as f64, natural.1 as f64);
    if !(w > 0.0 && h > 0.0) {
        return (1, 1);
    }
    let d = (dpr as f64).clamp(1.0, 4.0);
    let fit = (viewport.0 as f64 / w).min(viewport.1 as f64 / h).min(1.0);
    let budget = pixel_budget as f64;
    let scale = (fit * d)
        .min(MAX_RASTER_SIDE as f64 / w)
        .min(MAX_RASTER_SIDE as f64 / h)
        .min((budget / (w * h)).sqrt());
    let mut rw = ((w * scale).floor() as u64).max(1);
    let mut rh = ((h * scale).floor() as u64).max(1);
    if rw * rh > pixel_budget {
        if w > h {
            rw = (pixel_budget / rh).max(1);
        } else {
            rh = (pixel_budget / rw).max(1);
        }
    }
    (rw as u32, rh as u32)
}

/// Retained byte estimate of a prepared SVG: `2 x source length + 1024 + raster w x h x 8`.
pub fn estimated_bytes(source_len: usize, raster: (u32, u32)) -> usize {
    2 * source_len + 1024 + raster.0 as usize * raster.1 as usize * 8
}

/// Raster size of the enlarged variant for the full view, or `None` to keep the current raster
/// (the budget cannot hold even the current pixel count).
///
/// With budget `A`: `pixels = (A - (2 x source length + 1024)) / 8`; below the current raster's
/// pixel count the current raster is kept; otherwise the pixel budget is capped at
/// 2 x 1,048,576 and the raster size is computed for the 1600 x 1000 reference viewport.
pub fn enlarged_raster(
    natural: (f32, f32),
    source_len: usize,
    current: (u32, u32),
    dpr: f32,
    budget_bytes: usize,
) -> Option<(u32, u32)> {
    let fixed = 2 * source_len + 1024;
    let pixels = budget_bytes.saturating_sub(fixed) as u64 / 8;
    if pixels < current.0 as u64 * current.1 as u64 {
        return None;
    }
    Some(raster_size(
        natural,
        FULL_VIEW_VIEWPORT,
        dpr,
        pixels.min(ENLARGED_PIXEL_CAP),
    ))
}

/// Wraps a sanitized document in an outer SVG declared at half the raster size with a view box
/// of the natural size.
pub fn wrap(sanitized: &str, natural: (f32, f32), raster: (u32, u32)) -> String {
    let inner = match sanitized.find("<svg") {
        Some(start) => &sanitized[start..],
        None => sanitized,
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\">{}</svg>",
        raster.0 as f32 / 2.0,
        raster.1 as f32 / 2.0,
        natural.0,
        natural.1,
        inner
    )
}

fn svg_error(error: impl std::fmt::Display) -> MediaError {
    MediaError::Svg(error.to_string())
}

/// Rasterizes a sanitized document at `raster` pixels.
pub fn rasterize(
    sanitized: &str,
    natural: (f32, f32),
    raster: (u32, u32),
    renderer: &SvgRenderer,
) -> Result<Arc<vespertine::RenderImage>, MediaError> {
    let wrapped = wrap(sanitized, natural, raster);
    renderer
        .render_single_frame(wrapped.as_bytes(), 1.0)
        .map_err(svg_error)
}

/// A new media with the same sanitized source rasterized at `raster`.
pub(crate) fn with_raster(
    media: &DecodedMedia,
    vector: &VectorSource,
    raster: (u32, u32),
    renderer: &SvgRenderer,
) -> Result<DecodedMedia, MediaError> {
    let image = rasterize(&vector.sanitized, media.natural, raster, renderer)?;
    Ok(DecodedMedia {
        image,
        natural: media.natural,
        retained_bytes: estimated_bytes(vector.sanitized.len(), raster),
        encoded: None,
        vector: Some(VectorSource {
            sanitized: vector.sanitized.clone(),
            raster,
        }),
    })
}

/// Sanitizes `bytes` and reads the declared logical size.
pub fn sanitize(
    bytes: &[u8],
    cap: usize,
    renderer: &SvgRenderer,
) -> Result<(Arc<str>, (f32, f32)), MediaError> {
    if bytes.len() > cap {
        return Err(MediaError::TooLarge {
            bytes: bytes.len(),
            cap,
        });
    }
    let limits = UntrustedImageLimits {
        max_input_bytes: cap,
        max_side: MAX_DECLARED_SIDE,
        max_decoder_alloc: super::decode::MAX_DECODER_ALLOC,
    };
    let sanitized = renderer
        .prepare_untrusted_svg(bytes, &limits)
        .map_err(svg_error)?;
    if sanitized.len() > cap {
        return Err(MediaError::TooLarge {
            bytes: sanitized.len(),
            cap,
        });
    }
    let tree = usvg::Tree::from_str(&sanitized, &usvg::Options::default()).map_err(svg_error)?;
    let size = tree.size();
    let natural = (size.width(), size.height());
    if !(natural.0.is_finite() && natural.1.is_finite() && natural.0 > 0.0 && natural.1 > 0.0) {
        return Err(MediaError::Svg("empty declared size".into()));
    }
    Ok((Arc::from(sanitized), natural))
}

/// Decodes vector `bytes` with the first raster of `kind`.
pub(crate) fn decode(
    bytes: &[u8],
    cap: usize,
    kind: PreviewKind,
    renderer: &SvgRenderer,
) -> Result<DecodedMedia, MediaError> {
    let (sanitized, natural) = sanitize(bytes, cap, renderer)?;
    let (viewport, budget) = match kind {
        PreviewKind::Full => (PREVIEW_VIEWPORT, PREVIEW_PIXEL_BUDGET),
        PreviewKind::Icon => (ICON_VIEWPORT, ICON_PIXEL_BUDGET),
    };
    let raster = raster_size(natural, viewport, DEFAULT_DPR, budget);
    let image = rasterize(&sanitized, natural, raster, renderer)?;
    Ok(DecodedMedia {
        image,
        natural,
        retained_bytes: estimated_bytes(sanitized.len(), raster),
        encoded: None,
        vector: Some(VectorSource { sanitized, raster }),
    })
}
