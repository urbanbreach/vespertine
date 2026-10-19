//! Theme roles of the navigation chrome (tab strips, segmented controls, toasts).
//!
//! Every colour of CMP-017 and CMP-018 comes from a [`NavTheme`]. The shared roles (text, muted
//! text, accent, danger, popup surface, border, corner radius) are those of
//! [`ComponentTokens`](crate::tokens::ComponentTokens), so the navigation chrome follows the same
//! mapping of the active THM-003 theme as the other components; the strip background (the
//! chrome shell) and the success colour come from the theme directly. Without an installed theme
//! a neutral dark set is used. Owners may also hand a theme to a component explicitly.

use crate::tokens::ComponentTokens;
use vespertine::{App, Hsla, Pixels, rgb};
use vespertine_theme::appearance::try_theme_of;

/// The theme roles the navigation components use.
#[derive(Clone, Debug, PartialEq)]
pub struct NavTheme {
    /// Opaque backing of the strip.
    pub background: Hsla,
    /// Primary text: selected labels, toast messages.
    pub text: Hsla,
    /// Muted text: unselected labels, the unsaved dot, info icons.
    pub muted: Hsla,
    /// Accent: the focus-visible ring.
    pub accent: Hsla,
    /// Danger status: error icons.
    pub danger: Hsla,
    /// Success status: success icons.
    pub success: Hsla,
    /// Popup surface fill (tooltips, toast cards).
    pub popup_surface: Hsla,
    /// Popup surface border.
    pub popup_border: Hsla,
    /// Popup surface corner radius.
    pub popup_radius: Pixels,
}

impl NavTheme {
    /// The roles of `tokens`, with the strip background and the success colour given.
    pub fn from_tokens(tokens: &ComponentTokens, background: Hsla, success: Hsla) -> Self {
        Self {
            background,
            text: tokens.text,
            muted: tokens.text_muted,
            accent: tokens.focus_ring,
            danger: tokens.danger,
            success,
            popup_surface: tokens.surface,
            popup_border: tokens.border,
            popup_radius: tokens.surface_radius,
        }
    }

    /// The neutral dark default, independent of any installed theme.
    pub fn neutral() -> Self {
        Self::from_tokens(
            &ComponentTokens::neutral(),
            rgb(0x181A1F).into(),
            rgb(0x58B368).into(),
        )
    }

    /// The active theme of the application, or the neutral default before the theme system is
    /// initialized.
    pub fn of(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self::from_tokens(
                &ComponentTokens::from_theme(theme),
                theme.colors.shell.to_hsla(),
                theme.colors.success.to_hsla(),
            ),
            None => Self::neutral(),
        }
    }

    /// A translucent wash of the neutral text tint with the given alpha (selected 10 %, hover
    /// 6 %, close slot hover 12 %).
    pub fn wash(&self, alpha: f32) -> Hsla {
        Hsla {
            a: alpha,
            ..self.text
        }
    }
}

impl Default for NavTheme {
    fn default() -> Self {
        Self::neutral()
    }
}
