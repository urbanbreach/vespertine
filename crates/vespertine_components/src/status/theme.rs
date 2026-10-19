//! Theme roles of the status kit (CMP-023 R2, R4, R12) and the shared pulse clock access
//! (R13, R14).

use std::time::Duration;

use vespertine::{AnimationCadence, App, Hsla, Window, hsla};
use vespertine_theme::appearance::try_theme_of;
use vespertine_theme::theme::{Appearance, ThemeRegistry, ThemeVariant};

use super::motion::subtle_pulse;

/// The colors the status components paint with.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusColors {
    /// Primary text (the foreground ink of skeleton tints and wave cells).
    pub text: Hsla,
    /// Muted text.
    pub muted: Hsla,
    /// Faint text.
    pub faint: Hsla,
    /// The accent (fills, matrix and glyph tints).
    pub accent: Hsla,
    /// Success status.
    pub success: Hsla,
    /// The muted track fill of determinate bars.
    pub track: Hsla,
    /// Hairlines.
    pub border: Hsla,
    /// Danger status.
    pub danger: Hsla,
    /// Hover wash.
    pub hover: Hsla,
}

impl StatusColors {
    /// The roles of a theme variant.
    pub fn from_variant(variant: &ThemeVariant) -> Self {
        Self {
            text: variant.colors.text.to_hsla(),
            muted: variant.colors.text_muted.to_hsla(),
            faint: variant.colors.text_faint.to_hsla(),
            accent: variant.accent.primary.to_hsla(),
            success: variant.colors.success.to_hsla(),
            track: variant.colors.border.to_hsla(),
            border: variant.colors.border.to_hsla(),
            danger: variant.colors.danger.to_hsla(),
            hover: variant.colors.hover.to_hsla(),
        }
    }

    /// The roles of the active theme (the built-in dark variant before a theme is installed).
    pub fn active(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self::from_variant(&theme.variant),
            None => {
                let registry = ThemeRegistry::builtin();
                Self::from_variant(registry.first_variant(Appearance::Dark))
            }
        }
    }
}

/// The phase of a decorative pulse and whether motion is reduced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pulse {
    /// Phase in `[0, 1)` over the requested period; a static 0 when motion is reduced.
    pub phase: f32,
    /// Whether motion is reduced (no lease was taken and no repeating frame is scheduled).
    pub reduced: bool,
}

/// Reads the shared pulse clock for the view being painted: renews its 300 ms lease at the
/// normal stride, or at stride 2 for indicators on coarse chrome. Under reduced motion the
/// phase is a static 0 and no lease is taken.
pub fn pulse(window: &mut Window, cx: &mut App, period: Duration, coarse: bool) -> Pulse {
    let reduced = cx.reduce_motion();
    let cadence = if coarse {
        AnimationCadence::Slow
    } else {
        AnimationCadence::Normal
    };
    Pulse {
        phase: window.animation_phase(cx, period, cadence),
        reduced,
    }
}

/// What a matrix spinner or mini glyph draws (R14).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Activity {
    /// Full motion: the chase over the 750 ms clock phase.
    Chase(f32),
    /// Reduced motion: every cell at one uniform opacity `0.6 + 0.2 x wave(phase)` (phase 0, so
    /// a flat 0.6, when the reduction is explicit or the window is paused).
    Uniform(f32),
}

/// Reads the activity-indicator clock: the chase under normal motion; under system-only
/// reduction a slow uniform pulse at stride 2; otherwise a flat 0.6 with no repeating frames.
pub fn activity(window: &mut Window, cx: &mut App) -> Activity {
    if !cx.reduce_motion() {
        return Activity::Chase(window.animation_phase_normal(cx, super::motion::FAST_PERIOD));
    }
    let indicator = window.activity_indicator(cx);
    Activity::Uniform(subtle_pulse(indicator.phase))
}

/// A foreground color with its alpha replaced.
pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    hsla(color.h, color.s, color.l, alpha)
}
