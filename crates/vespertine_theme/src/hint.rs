//! Image-derived palette hints (THM-004 R14-R16): extraction from decoded pixels, the
//! asynchronous hint service and the tint recipe.

use crate::{
    accent::derive_accent,
    appearance,
    color::Color,
    theme::{ThemeVariant, harden_variant_foregrounds},
};
use anyhow::{Context as _, Result};
use std::{path::PathBuf, sync::Arc};
use vespertine::{App, AppContext as _, Global, SharedString};

/// Pixels with alpha below this are ignored.
pub const ALPHA_CUTOFF: u8 = 128;

/// The longest side of the thumbnail the service extracts from.
pub const THUMBNAIL_SIZE: u32 = 64;

/// Extracts the representative color of RGBA pixels (4 bytes per pixel; a trailing partial
/// pixel is ignored). Pixels with alpha < 128 are ignored. Every other pixel adds the weight
/// `(0.2 + s^2) * alpha / 255`, with `s = (max - min) / max(max, 1)`, to its 4096-bin
/// quantization (4 bits per channel); the result is the weighted mean of the heaviest bin (ties
/// go to the highest bin index), rounded per channel, opaque. Returns `None` when no pixel
/// counted.
pub fn extract(pixels: &[u8]) -> Option<Color> {
    // Per bin: total weight and weight-multiplied R, G, B sums.
    let mut bins = vec![[0.0f64; 4]; 4096];
    let mut counted = false;
    for pixel in pixels.chunks_exact(4) {
        let [r, g, b, a] = [pixel[0], pixel[1], pixel[2], pixel[3]];
        if a < ALPHA_CUTOFF {
            continue;
        }
        let high = r.max(g).max(b);
        let low = r.min(g).min(b);
        let saturation = f64::from(high - low) / f64::from(high.max(1));
        let weight = (0.2 + saturation * saturation) * f64::from(a) / 255.0;
        let index = (usize::from(r >> 4) << 8) | (usize::from(g >> 4) << 4) | usize::from(b >> 4);
        let bin = &mut bins[index];
        bin[0] += weight;
        bin[1] += weight * f64::from(r);
        bin[2] += weight * f64::from(g);
        bin[3] += weight * f64::from(b);
        counted = true;
    }
    if !counted {
        return None;
    }
    let mut best = 0usize;
    for (index, bin) in bins.iter().enumerate() {
        if bin[0] > 0.0 && bin[0] >= bins[best][0] {
            best = index;
        }
    }
    let bin = &bins[best];
    let channel = |sum: f64| (sum / bin[0]).round() as u8;
    Some(Color::rgb(
        channel(bin[1]),
        channel(bin[2]),
        channel(bin[3]),
    ))
}

/// Decodes encoded image bytes, reduces them to a thumbnail of at most 64 x 64 pixels and
/// extracts the hint.
pub fn extract_from_encoded(bytes: &[u8]) -> Result<Option<Color>> {
    let image = image::load_from_memory(bytes).context("decoding image")?;
    let thumbnail = image.thumbnail(THUMBNAIL_SIZE, THUMBNAIL_SIZE).to_rgba8();
    Ok(extract(thumbnail.as_raw()))
}

/// The pixels of a hint request.
#[derive(Clone, Debug)]
pub enum HintSource {
    /// An image file, read and decoded on the background executor.
    File(PathBuf),
    /// Encoded image bytes.
    Bytes(Arc<[u8]>),
}

/// An image the hint is computed for. `key` identifies the image: a result is applied only if
/// the image with that key is still the current one.
#[derive(Clone, Debug)]
pub struct HintImage {
    /// Identity of the image.
    pub key: SharedString,
    /// Where the pixels come from.
    pub source: HintSource,
}

impl HintImage {
    /// An image file; the path is its identity.
    pub fn file(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        Self {
            key: path.to_string_lossy().into_owned().into(),
            source: HintSource::File(path),
        }
    }

    /// Encoded bytes with a caller-chosen identity.
    pub fn bytes(key: impl Into<SharedString>, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            key: key.into(),
            source: HintSource::Bytes(bytes.into()),
        }
    }
}

/// Turns a source into a hint; runs on the background executor.
pub type HintExtractor = Arc<dyn Fn(&HintSource) -> Result<Option<Color>> + Send + Sync>;

fn default_extractor() -> HintExtractor {
    Arc::new(|source| match source {
        HintSource::File(path) => {
            let bytes = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
            extract_from_encoded(&bytes)
        }
        HintSource::Bytes(bytes) => extract_from_encoded(bytes),
    })
}

struct HintService {
    current: Option<SharedString>,
    pending: Option<SharedString>,
    extractor: HintExtractor,
}

impl Global for HintService {}

fn service(cx: &mut App) -> &mut HintService {
    if !cx.has_global::<HintService>() {
        cx.set_global(HintService {
            current: None,
            pending: None,
            extractor: default_extractor(),
        });
    }
    cx.global_mut::<HintService>()
}

/// Replaces the decoding step of the service (default: decode the image and extract from a
/// 64 x 64 thumbnail). The extractor always runs on the background executor.
pub fn set_hint_extractor(cx: &mut App, extractor: HintExtractor) {
    service(cx).extractor = extractor;
}

/// Sets (or, with `None`, removes) the image the hint derives from, and starts the background
/// extraction. A request whose extraction is already pending is not started twice. Removing
/// the image clears the hint; replacing it keeps the previous hint until the new result
/// arrives, and decode errors or empty results keep it (THM-004 R15).
pub fn set_hint_image(cx: &mut App, image: Option<HintImage>) {
    let Some(image) = image else {
        let service = service(cx);
        service.current = None;
        service.pending = None;
        appearance::set_hint(cx, None);
        return;
    };
    let service = service(cx);
    service.current = Some(image.key.clone());
    if service.pending.as_ref() == Some(&image.key) {
        return;
    }
    service.pending = Some(image.key.clone());
    let extractor = service.extractor.clone();
    let key = image.key;
    let source = image.source;
    let task = cx.background_spawn(async move { extractor(&source) });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| deliver(cx, key, result));
    })
    .detach();
}

fn deliver(cx: &mut App, key: SharedString, result: Result<Option<Color>>) {
    let service = service(cx);
    if service.pending.as_ref() == Some(&key) {
        service.pending = None;
    }
    if service.current.as_ref() != Some(&key) {
        return;
    }
    match result {
        Ok(Some(color)) => appearance::set_hint(cx, Some(color)),
        Ok(None) => log::debug!("palette hint: image {key} has no countable pixels"),
        Err(error) => log::warn!("palette hint: image {key} could not be decoded: {error:#}"),
    }
}

/// The tint recipe constants (themeable defaults, THM-004 R16).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TintRecipe {
    /// Mix of the hint toward black for dark variants.
    pub dark_neutral_mix: f64,
    /// Mix of the hint toward white for light variants.
    pub light_neutral_mix: f64,
    /// Mix of each surface toward the tint.
    pub surface_mix: f64,
    /// Alpha of `hover`.
    pub hover_alpha: f64,
    /// Alpha of `active`.
    pub active_alpha: f64,
    /// Alpha of `border`.
    pub border_alpha: f64,
    /// Alpha of `borderStrong`.
    pub border_strong_alpha: f64,
}

impl Default for TintRecipe {
    fn default() -> Self {
        Self {
            dark_neutral_mix: 0.88,
            light_neutral_mix: 0.94,
            surface_mix: 0.4,
            hover_alpha: 0.09,
            active_alpha: 0.15,
            border_alpha: 0.14,
            border_strong_alpha: 0.30,
        }
    }
}

/// Applies the tint recipe for `hint` to a variant: surfaces (and the terminal background)
/// move toward the tint, the accent roles are re-derived from the hint over the new background,
/// washes and borders follow the accent, and text roles are re-hardened against the tinted
/// surfaces. Semantic colors (danger, success, warning, syntax, diff) are unchanged.
pub fn tint_variant(variant: &ThemeVariant, hint: Color, recipe: &TintRecipe) -> ThemeVariant {
    let mut tinted = variant.clone();
    let dark = variant.appearance.is_dark();
    let (neutral, amount) = if dark {
        (Color::BLACK, recipe.dark_neutral_mix)
    } else {
        (Color::WHITE, recipe.light_neutral_mix)
    };
    let tint = hint.mix(neutral, amount);
    let surface = |color: Color| color.mix(tint, recipe.surface_mix);
    let colors = &mut tinted.colors;
    colors.background = surface(colors.background);
    colors.shell = surface(colors.shell);
    colors.raised = surface(colors.raised);
    colors.card = surface(colors.card);
    colors.dialog = surface(colors.dialog);
    colors.overlay = surface(colors.overlay);
    colors.input = surface(colors.input);
    tinted.accent = derive_accent(hint, tinted.colors.background, variant.appearance);
    let primary = tinted.accent.primary;
    tinted.colors.hover = primary.with_alpha(recipe.hover_alpha);
    tinted.colors.active = primary.with_alpha(recipe.active_alpha);
    tinted.colors.border = primary.with_alpha(recipe.border_alpha);
    tinted.colors.border_strong = primary.with_alpha(recipe.border_strong_alpha);
    tinted.terminal.background = surface(tinted.terminal.background);
    tinted.terminal.selection = tinted.accent.selection;
    harden_variant_foregrounds(&mut tinted);
    tinted
}
