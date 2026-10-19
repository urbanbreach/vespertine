//! Swatch scenes shared by the `color_swatches` example and the pixel tests
//! (FID-THM-001 and FID-THM-002).

#![allow(dead_code)]

use serde_json::json;
use vespertine::{
    Context, IntoElement, Render, Rgba, Styled as _, Window, css_color_to_rgba, div, prelude::*,
    px, rgb,
};
use vespertine_theme::document::DocumentTheme;

/// How a swatch resolves its fill text into a color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorPath {
    /// The text goes through the shared parser straight into an element style color.
    ElementStyle,
    /// The text is a component theme override (`bg` token) and the resolved token is used.
    ThemeOverride,
}

impl ColorPath {
    pub fn resolve(self, text: &str) -> Rgba {
        match self {
            ColorPath::ElementStyle => css_color_to_rgba(text).expect("swatch color text parses"),
            ColorPath::ThemeOverride => DocumentTheme::from_value(Some(&json!({ "bg": text }))).bg,
        }
    }
}

/// FID-THM-001: window 480 x 180, white background, twelve 40 x 40 swatches
/// (fill text, expected composited sRGB on white).
pub const THM_001_WINDOW: (f32, f32) = (480., 180.);
pub const THM_001_SWATCHES: [(&str, u32); 12] = [
    ("rgb(255 0 0)", 0xFF0000),
    ("oklch(0.62796 0.25768 29.23388)", 0xFF0000),
    ("rgba(255,0,0,0.5)", 0xFF8080),
    ("rebeccapurple", 0x663399),
    ("hsl(120 100% 25%)", 0x008000),
    ("#0f08", 0x77FF77),
    ("hwb(210 20% 30%)", 0x3373B3),
    ("hsv(30 100% 100%)", 0xFF8000),
    ("lab(60% 40 50)", 0xE17138),
    ("oklch(90% 0.4 40)", 0xFF0000),
    ("rgb(from #bad455 b r g / alpha)", 0x55BAD4),
    ("oklab(from #bad455 calc(l * 0.7) a b)", 0x708500),
];

/// Top-left of swatch `index` of FID-THM-001 in logical pixels.
pub fn thm_001_origin(index: usize) -> (f32, f32) {
    (
        20. + 80. * (index % 6) as f32,
        if index < 6 { 30. } else { 110. },
    )
}

/// A static scene of absolutely placed 40 x 40 swatches on a background.
pub struct SwatchScene {
    pub window: (f32, f32),
    pub background: Rgba,
    /// (left, top, fill)
    pub swatches: Vec<(f32, f32, Rgba)>,
}

impl SwatchScene {
    pub fn thm_001(path: ColorPath) -> Self {
        Self {
            window: THM_001_WINDOW,
            background: rgb(0xFFFFFF),
            swatches: THM_001_SWATCHES
                .iter()
                .enumerate()
                .map(|(index, (text, _))| {
                    let (x, y) = thm_001_origin(index);
                    (x, y, path.resolve(text))
                })
                .collect(),
        }
    }
}

/// FID-THM-002: window 460 x 100, background #202020, six swatches filled with resolved
/// theme tokens (resolved color, expected sRGB).
pub const THM_002_WINDOW: (f32, f32) = (460., 100.);

pub fn thm_002_swatches() -> Vec<(Rgba, u32)> {
    let dark = DocumentTheme::dark();
    let light = DocumentTheme::light();
    let red_accent = DocumentTheme::from_value(Some(&json!({"accent": "#FF0000"})));
    vec![
        (dark.bg, 0x060606),
        (light.bg, 0xFFFFFF),
        (dark.accent, 0x7C86FF),
        (red_accent.accent, 0xFF0000),
        (dark.text, 0xE5E5E5),
        (light.accent, 0x4F39F6),
    ]
}

impl SwatchScene {
    pub fn thm_002() -> Self {
        Self {
            window: THM_002_WINDOW,
            background: rgb(0x202020),
            swatches: thm_002_swatches()
                .into_iter()
                .enumerate()
                .map(|(index, (fill, _))| (20. + 70. * index as f32, 30., fill))
                .collect(),
        }
    }
}

impl Render for SwatchScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .w(px(self.window.0))
            .h(px(self.window.1))
            .bg(self.background)
            .children(self.swatches.iter().map(|&(x, y, fill)| {
                div()
                    .absolute()
                    .top(px(y))
                    .left(px(x))
                    .size(px(40.))
                    .bg(fill)
            }))
    }
}

/// Composites `fill` over `background` as the renderer does for an opaque window.
pub fn composite(fill: Rgba, background: Rgba) -> [u8; 3] {
    let channel = |f: f32, b: f32| ((f * fill.a + b * (1. - fill.a)) * 255.).round() as u8;
    [
        channel(fill.r, background.r),
        channel(fill.g, background.g),
        channel(fill.b, background.b),
    ]
}
