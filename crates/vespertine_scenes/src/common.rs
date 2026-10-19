//! Small helpers shared by the scenes.

use vespertine::{Hsla, hsla, rgb};

pub const INK: u32 = 0x1f2328;
pub const BORDER: u32 = 0xd0d7de;
pub const SURFACE: u32 = 0xf6f8fa;
pub const WHITE: u32 = 0xffffff;

pub const SANS: &str = "Noto Sans";
pub const MONO: &str = "JetBrainsMono Nerd Font";

/// sRGB hex color (0xRRGGBB) as an opaque color.
pub fn hex(value: u32) -> Hsla {
    rgb(value).into()
}

/// HSL color with the hue in whole degrees and saturation/lightness in `0.0..=1.0`.
pub fn hsl_deg(hue_degrees: u32, saturation: f32, lightness: f32) -> Hsla {
    hsla(
        (hue_degrees % 360) as f32 / 360.0,
        saturation,
        lightness,
        1.0,
    )
}
