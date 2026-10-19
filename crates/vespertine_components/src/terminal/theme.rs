//! Terminal colors (CMP-021 R10): the theme's terminal roles plus the fixed 6x6x6 cube and the
//! grayscale ramp.

use vespertine::{Rgba, SharedString};
use vespertine_theme::appearance::{self, ActiveTheme};
use vespertine_theme::builtin;
use vespertine_theme::theme::{Appearance, TypographyRoles};

use super::emulator::CellColor;
use super::geometry::{DEFAULT_FONT_SIZE, default_line_height};

/// Component levels of the color cube (identical in light and dark appearances).
pub const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
/// Alpha multiplier of dim text (R8).
pub const DIM_ALPHA: f32 = 0.6;
/// Alpha of the translucent cursor block, so the glyph beneath stays legible (R9).
pub const CURSOR_BLOCK_ALPHA: f32 = 0.55;
/// Bounds of the selection wash alpha: strictly between 0 and 0.5 (R7).
const SELECTION_ALPHA_RANGE: (f32, f32) = (0.1, 0.45);

/// Resolves an indexed color (R10): 0-15 from the theme palette `ansi`, 16-231 the color cube
/// (`n = index - 16`, red = levels[n / 36], green = levels[(n / 6) % 6], blue = levels[n % 6]),
/// 232-255 the grayscale ramp `8 + 10 x step`, mirrored (`23 - step`) in light appearance.
pub fn indexed_color(index: u8, ansi: &[Rgba; 16], dark: bool) -> Rgba {
    match index {
        0..=15 => ansi[usize::from(index)],
        16..=231 => {
            let n = usize::from(index - 16);
            rgb8(
                CUBE_LEVELS[n / 36],
                CUBE_LEVELS[(n / 6) % 6],
                CUBE_LEVELS[n % 6],
            )
        }
        232..=255 => {
            let step = index - 232;
            let step = if dark { step } else { 23 - step };
            let value = 8 + 10 * step;
            rgb8(value, value, value)
        }
    }
}

/// An opaque color from 8-bit channels.
pub fn rgb8(r: u8, g: u8, b: u8) -> Rgba {
    Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
}

/// The colors and font of a terminal surface.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalTheme {
    /// Default foreground.
    pub foreground: Rgba,
    /// The surface fill: the default background.
    pub background: Rgba,
    /// The translucent selection wash (alpha strictly between 0 and 0.5).
    pub selection: Rgba,
    /// The cursor color.
    pub cursor: Rgba,
    /// The 16 ANSI colors.
    pub ansi: [Rgba; 16],
    /// Whether the appearance is dark (the grayscale ramp mirrors in light appearance).
    pub dark: bool,
    /// The terminal font family.
    pub font_family: SharedString,
    /// The terminal font size in logical pixels.
    pub font_size: f32,
    /// Line height in logical pixels: `font_size x 18 / 13` by default.
    pub line_height: f32,
    /// Opacity of the surface fill (1 = opaque); explicit cell backgrounds always paint opaque.
    pub surface_opacity: f32,
}

impl TerminalTheme {
    /// The terminal roles of the resolved theme (THM-003), with its terminal font roles.
    pub fn from_active(theme: &ActiveTheme) -> Self {
        let mut resolved = Self::from_variant(&theme.variant);
        resolved.font_family = theme.typography.terminal_family.clone();
        resolved.font_size = theme.typography.terminal_size;
        resolved.line_height = default_line_height(resolved.font_size);
        resolved
    }

    /// The terminal roles of a theme variant, with the default terminal font.
    pub fn from_variant(variant: &vespertine_theme::theme::ThemeVariant) -> Self {
        let palette = &variant.terminal;
        let mut selection = Rgba::from(palette.selection);
        selection.a = selection
            .a
            .clamp(SELECTION_ALPHA_RANGE.0, SELECTION_ALPHA_RANGE.1);
        Self {
            foreground: palette.foreground.into(),
            background: palette.background.into(),
            selection,
            cursor: variant.accent.caret.into(),
            ansi: palette.ansi.map(Rgba::from),
            dark: variant.appearance == Appearance::Dark,
            font_family: "DejaVu Sans Mono".into(),
            font_size: DEFAULT_FONT_SIZE,
            line_height: default_line_height(DEFAULT_FONT_SIZE),
            surface_opacity: 1.0,
        }
    }

    /// The theme in effect for a context: the active theme when the theme system is
    /// initialized, else the built-in neutral dark variant.
    pub fn current(cx: &vespertine::App) -> Self {
        match appearance::try_theme_of(cx) {
            Some(theme) => Self::from_active(theme),
            None => {
                let mut theme = Self::from_variant(&builtin::neutral_variant(Appearance::Dark));
                if let Some(typography) = cx.try_global::<vespertine::Typography>() {
                    let roles = TypographyRoles::from_typography(typography);
                    theme.font_family = roles.terminal_family;
                    theme = theme.with_font_size(roles.terminal_size);
                }
                theme
            }
        }
    }

    /// The same theme with another font size; the line height keeps the 18 / 13 ratio.
    pub fn with_font_size(mut self, font_size: f32) -> Self {
        self.font_size = font_size;
        self.line_height = default_line_height(font_size);
        self
    }

    /// Resolves a cell color to paint color (R10).
    pub fn resolve(&self, color: CellColor) -> Rgba {
        match color {
            CellColor::DefaultForeground => self.foreground,
            CellColor::DefaultBackground => self.background,
            CellColor::Indexed(index) => indexed_color(index, &self.ansi, self.dark),
            CellColor::Rgb(r, g, b) => rgb8(r, g, b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine_theme::theme::ThemeRegistry;

    fn channels(color: Rgba) -> (u8, u8, u8) {
        let to = |value: f32| (value * 255.0).round() as u8;
        (to(color.r), to(color.g), to(color.b))
    }

    #[test]
    fn cube_is_identical_in_both_appearances() {
        let ansi = [rgb8(1, 2, 3); 16];
        for dark in [true, false] {
            assert_eq!(channels(indexed_color(16, &ansi, dark)), (0, 0, 0));
            assert_eq!(channels(indexed_color(21, &ansi, dark)), (0, 0, 255));
            assert_eq!(channels(indexed_color(196, &ansi, dark)), (255, 0, 0));
            assert_eq!(channels(indexed_color(231, &ansi, dark)), (255, 255, 255));
            assert_eq!(channels(indexed_color(110, &ansi, dark)), (135, 175, 215));
        }
    }

    #[test]
    fn grayscale_ramp_mirrors_in_light_appearance() {
        let ansi = [rgb8(1, 2, 3); 16];
        assert_eq!(channels(indexed_color(232, &ansi, true)), (8, 8, 8));
        assert_eq!(channels(indexed_color(255, &ansi, true)), (238, 238, 238));
        assert_eq!(channels(indexed_color(232, &ansi, false)), (238, 238, 238));
        assert_eq!(channels(indexed_color(255, &ansi, false)), (8, 8, 8));
    }

    #[test]
    fn low_indexes_and_selection_come_from_every_registered_theme() {
        let registry = ThemeRegistry::builtin();
        let mut checked = 0;
        for family in registry.families() {
            for variant in &family.variants {
                let theme = TerminalTheme::from_variant(variant);
                for index in 0..16u8 {
                    assert_eq!(
                        theme.resolve(CellColor::Indexed(index)),
                        Rgba::from(variant.terminal.ansi[usize::from(index)]),
                        "{} index {index}",
                        variant.id
                    );
                }
                assert!(theme.selection.a > 0.0 && theme.selection.a < 0.5);
                assert_eq!(theme.dark, variant.appearance == Appearance::Dark);
                checked += 1;
            }
        }
        assert!(checked >= 2);
    }

    #[test]
    fn selection_alpha_is_clamped_into_the_open_interval() {
        let mut variant = builtin::neutral_variant(Appearance::Dark);
        for alpha in [0, 255] {
            variant.terminal.selection.a = alpha;
            let theme = TerminalTheme::from_variant(&variant);
            assert!(theme.selection.a > 0.0 && theme.selection.a < 0.5);
        }
    }

    #[test]
    fn font_size_keeps_the_line_height_ratio() {
        let theme = TerminalTheme::from_variant(&builtin::neutral_variant(Appearance::Dark));
        assert!((theme.line_height - 18.0).abs() < 1e-4);
        let bigger = theme.with_font_size(26.0);
        assert!((bigger.line_height - 36.0).abs() < 1e-4);
    }
}
