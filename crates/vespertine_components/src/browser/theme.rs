//! Theme roles and metrics of the browser surface (CMP-022 R2).
//!
//! Every colour of the surface is a semantic role of the active THM-003 theme: [`BrowserTheme`]
//! maps the theme's colour table onto the nine roles the chrome uses. Without an installed theme
//! the roles come from the built-in neutral dark variant (the THM-002 tokens), never from values
//! private to the browser surface. An embedder may still hand an explicit [`BrowserTheme`] to
//! `BrowserSurface::set_theme`.

use vespertine::{App, Hsla, Pixels, px};
use vespertine_theme::{
    appearance::{ActiveTheme, try_theme_of},
    builtin::neutral_variant,
    theme::{Appearance, ColorTable},
};

/// The theme roles the browser surface uses.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowserTheme {
    /// Primary text (`text`).
    pub text: Hsla,
    /// Muted text: icons and secondary labels (`textMuted`).
    pub muted: Hsla,
    /// Faint text: hints (`textFaint`).
    pub faint: Hsla,
    /// Hairline borders (`border`).
    pub border: Hsla,
    /// Borders of focused or emphasized controls (`borderStrong`).
    pub strong_border: Hsla,
    /// Raised surfaces: toolbar, buttons, cards (`raised`).
    pub raised: Hsla,
    /// Opaque backing of the page area and the address field (`background`).
    pub page_background: Hsla,
    /// Validation border and message (`danger`).
    pub danger: Hsla,
    /// Translucent wash drawn over hovered controls (`hover`).
    pub hover: Hsla,
}

impl BrowserTheme {
    /// The roles of a colour table (THM-003 R2).
    pub fn from_colors(colors: &ColorTable) -> Self {
        Self {
            text: colors.text.to_hsla(),
            muted: colors.text_muted.to_hsla(),
            faint: colors.text_faint.to_hsla(),
            border: colors.border.to_hsla(),
            strong_border: colors.border_strong.to_hsla(),
            raised: colors.raised.to_hsla(),
            page_background: colors.background.to_hsla(),
            danger: colors.danger.to_hsla(),
            hover: colors.hover.to_hsla(),
        }
    }

    /// The roles of a resolved theme.
    pub fn from_theme(theme: &ActiveTheme) -> Self {
        Self::from_colors(&theme.colors)
    }

    /// The roles of the active theme, or of the built-in neutral dark variant when no theme is
    /// installed.
    pub fn current(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self::from_theme(theme),
            None => Self::default(),
        }
    }
}

impl Default for BrowserTheme {
    /// The roles of the built-in neutral dark variant (the THM-002 dark tokens).
    fn default() -> Self {
        Self::from_colors(&neutral_variant(Appearance::Dark).colors)
    }
}

/// Neutral default metrics of the chrome (R2, R12); every value is overridable.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowserMetrics {
    /// Height of the toolbar row.
    pub toolbar_height: Pixels,
    /// Horizontal padding of the toolbar.
    pub toolbar_padding: Pixels,
    /// Gap between toolbar items.
    pub toolbar_gap: Pixels,
    /// Side of the square toolbar buttons.
    pub button_size: Pixels,
    /// Size of the toolbar glyphs.
    pub glyph_size: Pixels,
    /// Corner radius of the toolbar buttons.
    pub button_radius: Pixels,
    /// Height of the address field.
    pub address_height: Pixels,
    /// Corner radius of the address field.
    pub address_radius: Pixels,
    /// Extra space left of the address field.
    pub address_margin_left: Pixels,
    /// Extra space right of the address field.
    pub address_margin_right: Pixels,
    /// Text size of the address field.
    pub address_text_size: Pixels,
    /// Line height of the address field.
    pub address_line_height: Pixels,
    /// Size of the globe glyph inside the address field.
    pub globe_size: Pixels,
    /// Side of the square go button.
    pub go_button_size: Pixels,
    /// Text size of the validation message.
    pub validation_text_size: Pixels,
}

impl Default for BrowserMetrics {
    fn default() -> Self {
        Self {
            toolbar_height: px(40.),
            toolbar_padding: px(8.),
            toolbar_gap: px(4.),
            button_size: px(28.),
            glyph_size: px(12.),
            button_radius: px(6.),
            address_height: px(24.),
            address_radius: px(4.),
            address_margin_left: px(0.),
            address_margin_right: px(0.),
            address_text_size: px(11.),
            address_line_height: px(16.),
            globe_size: px(12.),
            go_button_size: px(18.),
            validation_text_size: px(11.),
        }
    }
}
