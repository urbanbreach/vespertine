//! Accent derivation (THM-003 R13) and frosted tint coverage (THM-003 R17).

use crate::{
    color::Color,
    theme::{AccentRoles, Appearance},
};

/// Derives every accent role from a primary color `primary` over `background`.
pub fn derive_accent(primary: Color, background: Color, appearance: Appearance) -> AccentRoles {
    let dark = appearance.is_dark();
    let primary = primary.ensure_contrast(background, 3.0);
    let on = primary.best_on_color();
    let mut strong = primary;
    if on.contrast(strong) < 4.5 {
        strong = strong.ensure_contrast(on, 4.5);
    }
    let light_glyph = if dark {
        primary.mix(Color::WHITE, 0.28)
    } else {
        primary.mix(background, 0.18)
    };
    let deep_glyph = primary.mix(Color::BLACK, if dark { 0.18 } else { 0.26 });
    AccentRoles {
        primary,
        strong,
        wash: primary.with_alpha(if dark { 0.22 } else { 0.12 }),
        on,
        selection: primary.with_alpha(if dark { 0.35 } else { 0.24 }),
        caret: primary,
        activity: primary,
        glyph: [light_glyph, primary, deep_glyph],
    }
}

/// Frosted tint coverage: the first of 21 candidate alphas
/// `a_i = base + (1 - base) * i / 20` for which `text` (4.5) and `text_muted` (3.0) stay
/// readable on `tint` composited over the adverse backdrop (white for dark, black for light),
/// or `1.0` when none qualifies.
pub fn frosted_tint_alpha(
    tint: Color,
    base_alpha: f64,
    text: Color,
    text_muted: Color,
    appearance: Appearance,
) -> f64 {
    let backdrop = if appearance.is_dark() {
        Color::WHITE
    } else {
        Color::BLACK
    };
    let base = base_alpha.clamp(0.0, 1.0);
    for step in 0..=20 {
        let alpha = base + (1.0 - base) * f64::from(step) / 20.0;
        let composite = tint.with_alpha(alpha).blend_over(backdrop);
        if text.contrast(composite) >= 4.5 && text_muted.contrast(composite) >= 3.0 {
            return alpha;
        }
    }
    1.0
}
