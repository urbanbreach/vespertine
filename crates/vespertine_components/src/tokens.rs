//! Semantic token roles shared by the overlay and control components (CMP-001 R22, CMP-002 R2,
//! CMP-003 R4/R5, CMP-004 R11/R17, CMP-005 R10, CMP-006).
//!
//! Every component reads its colors from a [`ComponentTokens`] value. [`ComponentTokens::current`]
//! resolves the roles from the active THM-003 theme and falls back to the neutral dark set when
//! no theme is installed; callers may build and override a value themselves. Metrics are the
//! neutral defaults of the specifications and are all overridable through the public fields.

use vespertine::{App, Hsla, Pixels, px, rgb, rgba};
use vespertine_theme::appearance::try_theme_of;

/// Disabled opacity (CMP-002 A5: a disabled row has opacity 0.4).
pub const DISABLED_OPACITY: f32 = 0.4;

/// Colors of the semantic roles the controls and floating surfaces use.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentTokens {
    /// Resting fill of a button-like control surface.
    pub control_surface: Hsla,
    /// Fill of a hovered control (also the lifted trigger fill of a select).
    pub control_surface_hover: Hsla,
    /// Fill of a pressed control.
    pub control_surface_pressed: Hsla,
    /// Accent: selected fill, slider fill, switch on-track.
    pub accent: Hsla,
    /// Foreground on the accent (switch thumb).
    pub accent_contrast: Hsla,
    /// Rail, unselected track and hairline borders.
    pub border: Hsla,
    /// Borders of emphasized surfaces (cards, tooltips).
    pub border_strong: Hsla,
    /// Primary text.
    pub text: Hsla,
    /// Muted text (secondary label, tooltip text, range labels).
    pub text_muted: Hsla,
    /// Focus ring.
    pub focus_ring: Hsla,
    /// Danger (destructive action, error message).
    pub danger: Hsla,
    /// Floating surface fill (menus, popovers, selects).
    pub surface: Hsla,
    /// Raised surface (tooltip chip, hover card).
    pub surface_raised: Hsla,
    /// Dialog surface fill.
    pub surface_dialog: Hsla,
    /// Modal scrim: neutral default is black at 50% alpha.
    pub scrim: Hsla,
    /// Row wash of the highlighted row of a list.
    pub highlight: Hsla,
    /// Opacity of disabled controls and rows.
    pub disabled_opacity: f32,
    /// Corner radius of buttons.
    pub button_radius: Pixels,
    /// Corner radius of floating surfaces.
    pub surface_radius: Pixels,
}

impl Default for ComponentTokens {
    fn default() -> Self {
        Self::neutral()
    }
}

impl ComponentTokens {
    /// The neutral dark set, independent of any installed theme.
    pub fn neutral() -> Self {
        Self {
            control_surface: rgb(0x30343D).into(),
            control_surface_hover: rgb(0x3A3F49).into(),
            control_surface_pressed: rgb(0x262A31).into(),
            accent: rgb(0x4C8DF6).into(),
            accent_contrast: rgb(0xFFFFFF).into(),
            border: rgb(0x3F4450).into(),
            border_strong: rgb(0x6B7384).into(),
            text: rgb(0xF0F1F3).into(),
            text_muted: rgb(0xA3A9B5).into(),
            focus_ring: rgb(0x7AA7FF).into(),
            danger: rgb(0xE5675F).into(),
            surface: rgb(0x1A1A1A).into(),
            surface_raised: rgb(0x262A31).into(),
            surface_dialog: rgb(0x1F2127).into(),
            scrim: rgba(0x00000080).into(),
            highlight: rgba(0xFFFFFF1A).into(),
            disabled_opacity: DISABLED_OPACITY,
            button_radius: px(6.),
            surface_radius: px(8.),
        }
    }

    /// The roles of the active THM-003 theme, or the neutral set when none is installed.
    pub fn current(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self::from_theme(theme),
            None => Self::neutral(),
        }
    }

    /// Maps the semantic color table of a resolved theme onto the component roles.
    pub fn from_theme(theme: &vespertine_theme::appearance::ActiveTheme) -> Self {
        let colors = &theme.colors;
        let accent = &theme.accent;
        Self {
            control_surface: colors.raised.to_hsla(),
            control_surface_hover: colors.hover.blend_over(colors.raised).to_hsla(),
            control_surface_pressed: colors.active.blend_over(colors.raised).to_hsla(),
            accent: accent.strong.to_hsla(),
            accent_contrast: accent.on.to_hsla(),
            border: colors.border.to_hsla(),
            border_strong: colors.border_strong.to_hsla(),
            text: colors.text.to_hsla(),
            text_muted: colors.text_muted.to_hsla(),
            focus_ring: accent.primary.to_hsla(),
            danger: colors.danger.to_hsla(),
            surface: colors.overlay.to_hsla(),
            surface_raised: colors.raised.to_hsla(),
            surface_dialog: colors.dialog.to_hsla(),
            scrim: vespertine_theme::color::Color::BLACK
                .with_alpha(0.5)
                .to_hsla(),
            highlight: colors.hover.to_hsla(),
            ..Self::neutral()
        }
    }
}
