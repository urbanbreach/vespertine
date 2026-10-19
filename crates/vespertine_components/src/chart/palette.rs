//! Series colors and the categorical palette (CMP-024 R16).

use vespertine_theme::color::Color;
use vespertine_theme::theme::{Appearance, ThemeVariant};

/// The palette size.
pub const PALETTE_SIZE: usize = 6;

/// Candidate hues of palette slots 1-5 (teal, amber, rose, violet, lime, sky), in turns, with
/// their light (600) and dark (400) tones from the Tailwind CSS palette (MIT).
const CANDIDATES: [(f32, Color, Color); 6] = [
    (
        0.486,
        Color::rgb(0x0d, 0x94, 0x88),
        Color::rgb(0x2d, 0xd4, 0xbf),
    ),
    (
        0.106,
        Color::rgb(0xd9, 0x77, 0x06),
        Color::rgb(0xfb, 0xbf, 0x24),
    ),
    (
        0.964,
        Color::rgb(0xe1, 0x1d, 0x48),
        Color::rgb(0xfb, 0x71, 0x85),
    ),
    (
        0.728,
        Color::rgb(0x7c, 0x3a, 0xed),
        Color::rgb(0xa7, 0x8b, 0xfa),
    ),
    (
        0.236,
        Color::rgb(0x65, 0xa3, 0x0d),
        Color::rgb(0xa3, 0xe6, 0x35),
    ),
    (
        0.553,
        Color::rgb(0x02, 0x84, 0xc7),
        Color::rgb(0x38, 0xbd, 0xf8),
    ),
];

/// Saturation below which an accent counts as grey.
const GREY_SATURATION: f32 = 0.15;

fn hue_distance(a: f32, b: f32) -> f32 {
    let difference = (a - b).abs() % 1.0;
    difference.min(1.0 - difference)
}

/// The categorical series palette: slot 0 is the theme accent, slots 1-5 five fixed hues chosen
/// from six candidates by dropping the one whose hue is circularly closest to the accent
/// (the last candidate, sky, when the accent is nearly grey).
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesPalette {
    colors: [Color; PALETTE_SIZE],
}

impl SeriesPalette {
    /// The palette of a theme variant (its accent and appearance).
    pub fn from_theme(variant: &ThemeVariant) -> Self {
        Self::from_accent(variant.accent.primary, variant.appearance)
    }

    /// The palette for an accent in an appearance.
    pub fn from_accent(accent: Color, appearance: Appearance) -> Self {
        let hsla = accent.to_hsla();
        let dropped = if hsla.s < GREY_SATURATION {
            CANDIDATES.len() - 1
        } else {
            CANDIDATES
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    hue_distance(a.0, hsla.h).total_cmp(&hue_distance(b.0, hsla.h))
                })
                .map_or(CANDIDATES.len() - 1, |(index, _)| index)
        };
        let mut colors = [accent; PALETTE_SIZE];
        let tones = CANDIDATES
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != dropped)
            .map(|(_, candidate)| candidate);
        for (slot, candidate) in colors[1..].iter_mut().zip(tones) {
            *slot = if appearance.is_dark() {
                candidate.2
            } else {
                candidate.1
            };
        }
        Self { colors }
    }

    /// A caller-supplied palette.
    pub fn custom(colors: [Color; PALETTE_SIZE]) -> Self {
        Self { colors }
    }

    /// The color of series `index` (modulo the palette size).
    pub fn color(&self, index: usize) -> Color {
        self.colors[index % PALETTE_SIZE]
    }

    /// All six colors.
    pub fn colors(&self) -> &[Color; PALETTE_SIZE] {
        &self.colors
    }
}

/// A theme color role usable as a series color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeToken {
    /// Primary text.
    Text,
    /// Muted text.
    TextMuted,
    /// Faint text.
    TextFaint,
    /// Border.
    Border,
    /// Strong border.
    BorderStrong,
    /// The accent.
    Accent,
    /// Danger status.
    Danger,
    /// Warning status.
    Warning,
    /// Success status.
    Success,
}

impl ThemeToken {
    /// Parses a token name (`text`, `text_muted`, `accent`, `danger`, ...; `-` and `_` and case
    /// are ignored).
    pub fn parse(name: &str) -> Option<Self> {
        let normalized: String = name
            .chars()
            .filter(|c| *c != '-' && *c != '_')
            .map(|c| c.to_ascii_lowercase())
            .collect();
        Some(match normalized.as_str() {
            "text" => Self::Text,
            "textmuted" | "muted" => Self::TextMuted,
            "textfaint" | "faint" => Self::TextFaint,
            "border" => Self::Border,
            "borderstrong" => Self::BorderStrong,
            "accent" => Self::Accent,
            "danger" => Self::Danger,
            "warning" => Self::Warning,
            "success" => Self::Success,
            _ => return None,
        })
    }

    /// The token's color in a variant.
    pub fn resolve(self, variant: &ThemeVariant) -> Color {
        match self {
            Self::Text => variant.colors.text,
            Self::TextMuted => variant.colors.text_muted,
            Self::TextFaint => variant.colors.text_faint,
            Self::Border => variant.colors.border,
            Self::BorderStrong => variant.colors.border_strong,
            Self::Accent => variant.accent.primary,
            Self::Danger => variant.colors.danger,
            Self::Warning => variant.colors.warning,
            Self::Success => variant.colors.success,
        }
    }
}

/// An explicit series color: a theme token, a token mixed with another, or an sRGB value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SeriesColor {
    /// An sRGB value.
    Srgb(Color),
    /// A theme token.
    Token(ThemeToken),
    /// `a` mixed toward `b` by `amount` in `[0, 1]`.
    Mix {
        /// Base token.
        a: ThemeToken,
        /// Token mixed in.
        b: ThemeToken,
        /// Mix amount.
        amount: f64,
    },
}

impl SeriesColor {
    /// An opaque sRGB color.
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::Srgb(Color::rgb(r, g, b))
    }

    /// A token by name; `None` when the name is unknown.
    pub fn token(name: &str) -> Option<Self> {
        ThemeToken::parse(name).map(Self::Token)
    }

    /// The color in a theme variant.
    pub fn resolve(&self, variant: &ThemeVariant) -> Color {
        match *self {
            Self::Srgb(color) => color,
            Self::Token(token) => token.resolve(variant),
            Self::Mix { a, b, amount } => a.resolve(variant).mix(b.resolve(variant), amount),
        }
    }
}

impl From<Color> for SeriesColor {
    fn from(color: Color) -> Self {
        Self::Srgb(color)
    }
}

impl From<ThemeToken> for SeriesColor {
    fn from(token: ThemeToken) -> Self {
        Self::Token(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine_theme::theme::ThemeRegistry;

    fn dropped_hue(palette: &SeriesPalette, appearance: Appearance) -> Vec<usize> {
        // The candidate indexes absent from the palette's slots 1-5.
        (0..CANDIDATES.len())
            .filter(|index| {
                let tone = if appearance.is_dark() {
                    CANDIDATES[*index].2
                } else {
                    CANDIDATES[*index].1
                };
                !palette.colors()[1..].contains(&tone)
            })
            .collect()
    }

    #[test]
    fn a12_blue_accent_drops_sky_and_grey_drops_sky() {
        let blue = Color::rgb(0x2b, 0x7f, 0xff);
        let palette = SeriesPalette::from_accent(blue, Appearance::Dark);
        assert_eq!(palette.color(0), blue);
        assert_eq!(dropped_hue(&palette, Appearance::Dark), [5]);
        let grey = Color::rgb(0x80, 0x80, 0x80);
        let palette = SeriesPalette::from_accent(grey, Appearance::Dark);
        assert_eq!(dropped_hue(&palette, Appearance::Dark), [5]);
        // A red accent drops rose (hue 0.964).
        let red = Color::rgb(0xff, 0x20, 0x30);
        let palette = SeriesPalette::from_accent(red, Appearance::Light);
        assert_eq!(dropped_hue(&palette, Appearance::Light), [2]);
    }

    #[test]
    fn a12_light_uses_600_tones_and_dark_uses_400_tones() {
        let accent = Color::rgb(0x2b, 0x7f, 0xff);
        let light = SeriesPalette::from_accent(accent, Appearance::Light);
        let dark = SeriesPalette::from_accent(accent, Appearance::Dark);
        assert_eq!(light.color(1), Color::rgb(0x0d, 0x94, 0x88));
        assert_eq!(dark.color(1), Color::rgb(0x2d, 0xd4, 0xbf));
        assert_ne!(light, dark);
        assert_eq!(light.color(6), light.color(0));
    }

    #[test]
    fn palette_follows_the_theme_variant_and_tokens_resolve() {
        let registry = ThemeRegistry::builtin();
        let variant = registry.first_variant(Appearance::Dark);
        let palette = SeriesPalette::from_theme(variant);
        assert_eq!(palette.color(0), variant.accent.primary);
        let mixed = SeriesColor::Mix {
            a: ThemeToken::Text,
            b: ThemeToken::Accent,
            amount: 0.5,
        };
        assert_eq!(
            mixed.resolve(variant),
            variant.colors.text.mix(variant.accent.primary, 0.5)
        );
        assert_eq!(
            SeriesColor::token("text-muted"),
            Some(SeriesColor::Token(ThemeToken::TextMuted))
        );
        assert_eq!(SeriesColor::token("nope"), None);
    }
}
