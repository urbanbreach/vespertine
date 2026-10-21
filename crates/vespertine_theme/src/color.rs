//! 8-bit RGBA color model and the color helpers of THM-003 R3 and R14.
//!
//! Every helper rounds to the nearest 8-bit step with half away from zero, so results are exact
//! and reproducible. Amounts and alphas are `f64` so that decimal literals such as `0.28` do not
//! pick up `f32` representation error before rounding.

use serde::{Deserialize, Serialize};
use std::fmt;
use vespertine::{Hsla, Rgba, css_color_to_rgba};

/// A color with 8-bit channels. The text form is `#rrggbb`, or `#rrggbbaa` when not opaque.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color {
    /// Alpha channel (255 is opaque).
    pub a: u8,
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
}

/// Failure to parse the text form of a [`Color`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("invalid color `{0}`: expected #rgb, #rgba, #rrggbb or #rrggbbaa")]
pub struct ColorParseError(pub String);

impl Color {
    /// Opaque black.
    pub const BLACK: Color = Color::rgb(0, 0, 0);
    /// Opaque white.
    pub const WHITE: Color = Color::rgb(255, 255, 255);

    /// An opaque color.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// A color with explicit alpha.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Parses `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (any case, surrounding whitespace trimmed).
    pub fn parse(text: &str) -> Result<Self, ColorParseError> {
        let error = || ColorParseError(text.to_string());
        let trimmed = text.trim();
        // The core parser also accepts names and functional notations; only `#` + hex digits
        // are part of this grammar.
        let digits = trimmed.strip_prefix('#').ok_or_else(error)?;
        if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(error());
        }
        css_color_to_rgba(trimmed)
            .map(Self::from_rgba)
            .ok_or_else(error)
    }

    /// Lowercase hex text form (`#rrggbb`, or `#rrggbbaa` when alpha is not 255).
    pub fn to_hex(self) -> String {
        self.to_string()
    }

    /// Sets alpha to `round(clamp(alpha, 0, 1) * 255)`.
    pub fn with_alpha(self, alpha: f64) -> Self {
        Self {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    /// Composites this color over `background`, per channel
    /// `round(front * alpha + back * (1 - alpha))`; the result is opaque.
    pub fn blend_over(self, background: Color) -> Color {
        let alpha = f64::from(self.a) / 255.0;
        let channel = |front: u8, back: u8| {
            (f64::from(front) * alpha + f64::from(back) * (1.0 - alpha)).round() as u8
        };
        Color::rgb(
            channel(self.r, background.r),
            channel(self.g, background.g),
            channel(self.b, background.b),
        )
    }

    /// Interpolates every channel, including alpha, toward `other`:
    /// `round(a + (b - a) * amount)` with `amount` clamped to `[0, 1]`.
    pub fn mix(self, other: Color, amount: f64) -> Color {
        let amount = amount.clamp(0.0, 1.0);
        let channel = |from: u8, to: u8| {
            let from = f64::from(from);
            (from + (f64::from(to) - from) * amount).round() as u8
        };
        Color {
            r: channel(self.r, other.r),
            g: channel(self.g, other.g),
            b: channel(self.b, other.b),
            a: channel(self.a, other.a),
        }
    }

    /// Relative luminance of the color's RGB channels (alpha ignored): linearized channels
    /// weighted 0.2126, 0.7152, 0.0722.
    pub fn luminance(self) -> f64 {
        let linear = |channel: u8| -> f64 {
            let value = f64::from(channel) / 255.0;
            if value > 0.04045 {
                ((value + 0.055) / 1.055).powf(2.4)
            } else {
                value / 12.92
            }
        };
        let [red, green, blue] = [self.r, self.g, self.b].map(linear);
        0.2126 * red + 0.7152 * green + 0.0722 * blue
    }

    /// WCAG contrast ratio of this color as a foreground over `background`. A translucent
    /// foreground is first blended over the background.
    pub fn contrast(self, background: Color) -> f64 {
        let foreground = if self.a == 255 {
            self
        } else {
            self.blend_over(background)
        };
        let a = foreground.luminance();
        let b = background.luminance();
        let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
        (lighter + 0.05) / (darker + 0.05)
    }

    /// White when white has at least the contrast of black against this color, else black.
    pub fn best_on_color(self) -> Color {
        if Color::WHITE.contrast(self) >= Color::BLACK.contrast(self) {
            Color::WHITE
        } else {
            Color::BLACK
        }
    }

    /// Returns this color if it already meets `minimum` against `background`; otherwise moves it
    /// toward black or white (whichever contrasts more with the background) in 20 steps and
    /// returns the first candidate that meets the minimum, or the target itself.
    pub fn ensure_contrast(self, background: Color, minimum: f64) -> Color {
        if self.contrast(background) >= minimum {
            return self;
        }
        let target = if Color::BLACK.contrast(background) >= Color::WHITE.contrast(background) {
            Color::BLACK
        } else {
            Color::WHITE
        };
        for step in 1..=20 {
            let candidate = self.mix(target, f64::from(step) / 20.0);
            if candidate.contrast(background) >= minimum {
                return candidate;
            }
        }
        target
    }

    /// The paint color: HSL plus `alpha / 255`.
    pub fn to_hsla(self) -> Hsla {
        self.into()
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Color { r, g, b, a } = *self;
        if a == 255 {
            write!(f, "#{r:02x}{g:02x}{b:02x}")
        } else {
            write!(f, "#{r:02x}{g:02x}{b:02x}{a:02x}")
        }
    }
}

impl std::str::FromStr for Color {
    type Err = ColorParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Color::parse(text)
    }
}

impl TryFrom<String> for Color {
    type Error = ColorParseError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        Color::parse(&text)
    }
}

impl From<Color> for String {
    fn from(color: Color) -> Self {
        color.to_string()
    }
}

impl Color {
    /// Converts a paint-space [`Rgba`] to 8-bit channels, rounding each to the nearest step.
    pub fn from_rgba(color: Rgba) -> Self {
        let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        Self::rgba(
            channel(color.r),
            channel(color.g),
            channel(color.b),
            channel(color.a),
        )
    }
}

impl From<Color> for Rgba {
    fn from(color: Color) -> Self {
        Rgba {
            r: f32::from(color.r) / 255.0,
            g: f32::from(color.g) / 255.0,
            b: f32::from(color.b) / 255.0,
            a: f32::from(color.a) / 255.0,
        }
    }
}

impl From<Color> for Hsla {
    fn from(color: Color) -> Self {
        Rgba::from(color).into()
    }
}

/// Worst-case contrast of `color` over a set of backgrounds.
pub fn worst_contrast(color: Color, backgrounds: &[Color]) -> f64 {
    backgrounds
        .iter()
        .map(|background| color.contrast(*background))
        .fold(f64::INFINITY, f64::min)
}

/// Foreground hardening (THM-003 R15): raises `color` until its worst-case contrast over
/// `backgrounds` reaches `minimum`, trying the targets `[preferred, black, white]` in order with
/// 100 mix steps each. When no candidate reaches the minimum the candidate with the best
/// worst-case contrast is returned. A color that already qualifies is returned unchanged.
pub fn harden_foreground(
    color: Color,
    backgrounds: &[Color],
    minimum: f64,
    preferred: Option<Color>,
) -> Color {
    if worst_contrast(color, backgrounds) >= minimum {
        return color;
    }
    let mut best = color;
    let mut best_contrast = f64::NEG_INFINITY;
    for target in preferred.into_iter().chain([Color::BLACK, Color::WHITE]) {
        for step in 1..=100 {
            let candidate = color.mix(target, f64::from(step) / 100.0);
            let contrast = worst_contrast(candidate, backgrounds);
            if contrast >= minimum {
                return candidate;
            }
            if contrast > best_contrast {
                best_contrast = contrast;
                best = candidate;
            }
        }
    }
    best
}
