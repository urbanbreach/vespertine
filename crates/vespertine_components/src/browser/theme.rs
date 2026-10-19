//! Theme roles and metrics of the browser surface (CMP-022 R2).
//!
//! The surface reads every colour from a [`BrowserTheme`]; the defaults are a neutral dark set.
//! A theme system (THM-003) replaces only the source of the colours by building a
//! [`BrowserTheme`] and handing it to `BrowserSurface::set_theme`.

use vespertine::{Hsla, Pixels, px, rgb, rgba};

/// The theme roles the browser surface uses.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowserTheme {
    /// Primary text.
    pub text: Hsla,
    /// Muted text: icons and secondary labels.
    pub muted: Hsla,
    /// Faint text: hints.
    pub faint: Hsla,
    /// Hairline borders.
    pub border: Hsla,
    /// Borders of focused or emphasized controls.
    pub strong_border: Hsla,
    /// Raised surfaces: toolbar, buttons, cards.
    pub raised: Hsla,
    /// Opaque backing of the page area and the address field.
    pub page_background: Hsla,
    /// Validation border and message.
    pub danger: Hsla,
    /// Translucent wash drawn over hovered controls.
    pub hover: Hsla,
}

impl Default for BrowserTheme {
    fn default() -> Self {
        Self {
            text: rgb(0xF0F1F3).into(),
            muted: rgb(0xA3A9B5).into(),
            faint: rgb(0x727886).into(),
            border: rgb(0x3F4450).into(),
            strong_border: rgb(0x6B7384).into(),
            raised: rgb(0x30343D).into(),
            page_background: rgb(0x181A1F).into(),
            danger: rgb(0xE5675F).into(),
            hover: rgba(0xFFFFFF1A).into(),
        }
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
