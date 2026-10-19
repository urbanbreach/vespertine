//! Theme token roles of the badges, notices, review cards and checklist (CMP-028 Summary):
//! foreground, muted, faint and dim foreground, a neutral wash ("ink"), accent and on-accent,
//! warning, danger and busy tones, the popover surface, a monospace family and the spacing scale.

use vespertine::{App, Hsla, Pixels, SharedString, px, rgb};
use vespertine_theme::appearance::{ActiveTheme, try_theme_of};

/// Colors, fonts and spacing the CMP-028 components read. [`BadgeTokens::current`] resolves them
/// from the active THM-003 theme and falls back to a neutral dark set; hosts may build and
/// override a value themselves.
#[derive(Clone, Debug, PartialEq)]
pub struct BadgeTokens {
    /// Foreground (primary text).
    pub text: Hsla,
    /// Muted foreground.
    pub muted: Hsla,
    /// Faint foreground.
    pub faint: Hsla,
    /// Dim foreground: between muted and faint.
    pub dim: Hsla,
    /// The neutral wash tone: the foreground tone, applied at a stated opacity over the surface.
    pub ink: Hsla,
    /// Accent (solid).
    pub accent: Hsla,
    /// Foreground on the accent.
    pub on_accent: Hsla,
    /// Warning status tone.
    pub warning: Hsla,
    /// Muted warning companion (text).
    pub warning_muted: Hsla,
    /// Danger status tone.
    pub danger: Hsla,
    /// Muted danger companion (text).
    pub danger_muted: Hsla,
    /// Busy status tone (activity).
    pub busy: Hsla,
    /// Success status tone.
    pub success: Hsla,
    /// Popover surface fill.
    pub popover: Hsla,
    /// Popover hairline border.
    pub popover_border: Hsla,
    /// Hover background of secondary controls.
    pub element_hover: Hsla,
    /// The monospace family.
    pub mono_family: SharedString,
    /// The UI font scale (1 at the 16 px interface size).
    pub ui_scale: f32,
    /// The large spacing token (horizontal padding of review cards).
    pub spacing_large: Pixels,
}

impl Default for BadgeTokens {
    fn default() -> Self {
        Self::neutral()
    }
}

impl BadgeTokens {
    /// The neutral dark set, independent of any installed theme.
    pub fn neutral() -> Self {
        Self {
            text: rgb(0xF0F1F3).into(),
            muted: rgb(0xA3A9B5).into(),
            faint: rgb(0x727886).into(),
            dim: rgb(0x8A909C).into(),
            ink: rgb(0xF0F1F3).into(),
            accent: rgb(0x4C8DF6).into(),
            on_accent: rgb(0xFFFFFF).into(),
            warning: rgb(0xE5A23B).into(),
            warning_muted: rgb(0xD9B36A).into(),
            danger: rgb(0xE5675F).into(),
            danger_muted: rgb(0xE59A94).into(),
            busy: rgb(0x4C8DF6).into(),
            success: rgb(0x58B879).into(),
            popover: rgb(0x1A1A1A).into(),
            popover_border: rgb(0x3F4450).into(),
            element_hover: rgb(0x30343D).into(),
            mono_family: "DejaVu Sans Mono".into(),
            ui_scale: 1.0,
            spacing_large: px(16.),
        }
    }

    /// The roles of the active theme, or the neutral set when none is installed.
    pub fn current(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self::from_theme(theme),
            None => Self::neutral(),
        }
    }

    /// Maps a resolved theme onto the roles.
    pub fn from_theme(theme: &ActiveTheme) -> Self {
        let colors = &theme.colors;
        let accent = &theme.accent;
        Self {
            text: colors.text.to_hsla(),
            muted: colors.text_muted.to_hsla(),
            faint: colors.text_faint.to_hsla(),
            dim: colors.text_muted.mix(colors.text_faint, 0.5).to_hsla(),
            ink: colors.text.to_hsla(),
            accent: accent.strong.to_hsla(),
            on_accent: accent.on.to_hsla(),
            warning: colors.warning.to_hsla(),
            warning_muted: colors.warning_muted.to_hsla(),
            danger: colors.danger.to_hsla(),
            danger_muted: colors.danger_muted.to_hsla(),
            busy: accent.activity.to_hsla(),
            success: colors.success.to_hsla(),
            popover: colors.overlay.to_hsla(),
            popover_border: colors.border_strong.to_hsla(),
            element_hover: colors.hover.to_hsla(),
            mono_family: theme.typography.code_family.clone(),
            ui_scale: theme.typography.ui_size / 16.0,
            ..Self::neutral()
        }
    }

    /// The neutral wash at `opacity` (a fraction of the foreground tone over the surface).
    pub fn wash(&self, opacity: f32) -> Hsla {
        self.ink.opacity(opacity)
    }
}
