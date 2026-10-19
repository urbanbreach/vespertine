//! Count formatting, the activity count pill and the corner count badge (CMP-028 R4-R5).

use vespertine::{
    Bounds, Corners, ElementId, Font, FontFeatures, FontStyle, FontWeight, InteractiveElement,
    IntoElement, PaintQuad, ParentElement, Pixels, RenderOnce, SharedString, Styled, TextRun,
    Window, canvas, div, fill, point, px, size,
};

use super::tokens::BadgeTokens;

/// The default display cap: counts above it read `99+` (R5).
pub const COUNT_CAP: usize = 99;
/// Pill height (R4).
pub const PILL_HEIGHT: f32 = 16.0;
/// Status dot size.
pub const DOT_SIZE: f32 = 5.0;
/// Left (and default right) padding of the pill.
pub const PILL_PADDING: f32 = 6.0;
/// Gap between the dot and the label.
pub const PILL_GAP: f32 = 4.0;
/// Right padding reduction when the label ends in `+`: its thin arms leave a larger gap.
pub const PLUS_TRIM: f32 = 1.0;
/// Pill label size before the UI font scale.
pub const PILL_FONT_SIZE: f32 = 10.0;
/// Background opacity of the busy tone behind the pill.
pub const PILL_TONE_OPACITY: f32 = 0.16;
/// The middle of the digits above their baseline, in em, for the default monospace face.
pub const DIGIT_MIDDLE: f32 = 0.355;
/// Corner count badge height (R5).
pub const CORNER_HEIGHT: f32 = 13.0;
/// Corner count badge font size.
pub const CORNER_FONT_SIZE: f32 = 9.0;
/// Corner count badge horizontal padding.
pub const CORNER_PADDING: f32 = 3.0;
/// Corner count badge offset from the button's top-right corner.
pub const CORNER_OFFSET: f32 = -3.0;

/// The label for `count` under the default cap: exact up to 99, then `99+`. The count itself is
/// never altered; callers keep it for accessibility and tooltips.
pub fn count_label(count: usize) -> String {
    count_label_capped(count, COUNT_CAP)
}

/// The label for `count` with a configurable `cap`.
pub fn count_label_capped(count: usize, cap: usize) -> String {
    if count > cap {
        format!("{cap}+")
    } else {
        count.to_string()
    }
}

/// How a count is presented (R5): the label and the right padding that follows from it.
#[derive(Clone, Debug, PartialEq)]
pub struct CountDisplay {
    /// The unmodified count.
    pub count: usize,
    /// The displayed label.
    pub label: String,
    /// Whether the label is capped (ends in `+`).
    pub capped: bool,
    /// Right padding of the pill: reduced by 1 px when capped.
    pub pill_right_padding: f32,
    /// Right padding of the corner badge: 3 px, minus 1 px after `+`.
    pub corner_right_padding: f32,
}

/// Presents `count` under `cap`.
pub fn count_display(count: usize, cap: usize) -> CountDisplay {
    let capped = count > cap;
    let trim = if capped { PLUS_TRIM } else { 0.0 };
    CountDisplay {
        count,
        label: count_label_capped(count, cap),
        capped,
        pill_right_padding: PILL_PADDING - trim,
        corner_right_padding: CORNER_PADDING - trim,
    }
}

/// The opacity of the pulsing dot for the shared slow pulse `pulse_opacity` (ANM-004):
/// `lerp(0.4, 1.0, (pulse - 0.08) / 0.92)`, clamped.
pub fn dot_opacity(pulse_opacity: f32) -> f32 {
    let t = ((pulse_opacity - 0.08) / 0.92).clamp(0.0, 1.0);
    0.4 + (1.0 - 0.4) * t
}

/// The pill width, known before paint so cached views get a definite size: label width plus the
/// left padding, the dot, the gap and the right padding.
pub fn pill_width(label_width: f32, capped: bool) -> f32 {
    let right = if capped {
        PILL_PADDING - PLUS_TRIM
    } else {
        PILL_PADDING
    };
    label_width + PILL_PADDING + DOT_SIZE + PILL_GAP + right
}

/// Rounds a logical coordinate to a whole device pixel.
fn snap(value: Pixels, scale: f32) -> Pixels {
    px((f32::from(value) * scale).round() / scale)
}

fn mono_font(tokens: &BadgeTokens, weight: FontWeight) -> Font {
    Font {
        family: tokens.mono_family.clone(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight,
        style: FontStyle::Normal,
    }
}

/// An activity count pill: a pulsing status dot and the count of running items (R4).
#[derive(IntoElement)]
pub struct ActivityPill {
    key: ElementId,
    display: CountDisplay,
    pulse_opacity: f32,
    digit_middle: f32,
    tokens: BadgeTokens,
}

/// A pill for `count` running items keyed by the owner, or `None` at zero: callers draw nothing
/// at zero. `pulse_opacity` is the shared slow pulse clock's current opacity (ANM-004).
pub fn activity_pill(
    key: impl Into<ElementId>,
    count: usize,
    pulse_opacity: f32,
    tokens: &BadgeTokens,
) -> Option<ActivityPill> {
    (count > 0).then(|| ActivityPill {
        key: key.into(),
        display: count_display(count, COUNT_CAP),
        pulse_opacity,
        digit_middle: DIGIT_MIDDLE,
        tokens: tokens.clone(),
    })
}

impl ActivityPill {
    /// Uses measured digit metrics (the middle of the digits above the baseline, in em)
    /// instead of the default monospace constant.
    pub fn digit_middle(mut self, em: f32) -> Self {
        self.digit_middle = em;
        self
    }

    /// The configurable cap of the label.
    pub fn cap(mut self, cap: usize) -> Self {
        self.display = count_display(self.display.count, cap);
        self
    }
}

impl RenderOnce for ActivityPill {
    fn render(self, window: &mut Window, _cx: &mut vespertine::App) -> impl IntoElement {
        let tokens = self.tokens;
        let font_size = px(PILL_FONT_SIZE * tokens.ui_scale);
        let font = mono_font(&tokens, FontWeight::MEDIUM);
        let line_height = px(PILL_HEIGHT);
        let label: SharedString = self.display.label.clone().into();
        let run = TextRun {
            len: label.len(),
            font: font.clone(),
            color: tokens.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let system = window.text_system().clone();
        let line = system.shape_line(label, font_size, &[run], None);
        let width = pill_width(f32::from(line.width), self.display.capped);
        let font_id = system.resolve_font(&font);
        let baseline_offset = system.baseline_offset(font_id, font_size, line_height);
        let digit_middle = self.digit_middle;
        let busy = tokens.busy;
        let dot_alpha = dot_opacity(self.pulse_opacity);
        div()
            .id(self.key)
            .w(px(width))
            .h(line_height)
            .flex_none()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds: Bounds<Pixels>, _, window, cx| {
                        let scale = window.scale_factor();
                        window.paint_quad(PaintQuad {
                            corner_radii: Corners::all(bounds.size.height / 2.0),
                            ..fill(bounds, busy.opacity(PILL_TONE_OPACITY))
                        });
                        // The baseline is snapped to a device pixel and the dot hangs off the
                        // digits' own vertical middle, in whole device pixels.
                        let baseline = snap(bounds.top() + baseline_offset, scale);
                        let origin_y = baseline - baseline_offset;
                        let dot = snap(px(DOT_SIZE), scale).max(px(1.0 / scale));
                        let middle = baseline - font_size * digit_middle;
                        let dot_bounds = Bounds::new(
                            point(
                                snap(bounds.left() + px(PILL_PADDING), scale),
                                snap(middle - dot / 2.0, scale),
                            ),
                            size(dot, dot),
                        );
                        window.paint_quad(PaintQuad {
                            corner_radii: Corners::all(dot / 2.0),
                            ..fill(dot_bounds, busy.opacity(dot_alpha))
                        });
                        let text_x = bounds.left() + px(PILL_PADDING + DOT_SIZE + PILL_GAP);
                        crate::layout::paint_left_aligned(
                            &line,
                            point(text_x, origin_y),
                            line_height,
                            window,
                            cx,
                        );
                    },
                )
                .size_full(),
            )
    }
}

/// A count badge for the top-right corner of a toolbar button (R5): height 13, digits at 9 px
/// semibold in the on-accent color, 3 px padding either side (minus 1 px after `+`), offset
/// (-3, -3), static, painted so the digits are centered exactly. Place it as a child of a
/// `relative` button; it never changes the button's size or position.
#[derive(IntoElement)]
pub struct CornerCountBadge {
    display: CountDisplay,
    tokens: BadgeTokens,
}

/// A corner count badge for `count` under the default cap.
pub fn corner_count_badge(count: usize, tokens: &BadgeTokens) -> CornerCountBadge {
    CornerCountBadge {
        display: count_display(count, COUNT_CAP),
        tokens: tokens.clone(),
    }
}

impl RenderOnce for CornerCountBadge {
    fn render(self, window: &mut Window, _cx: &mut vespertine::App) -> impl IntoElement {
        let tokens = self.tokens;
        let font_size = px(CORNER_FONT_SIZE * tokens.ui_scale);
        let font = mono_font(&tokens, FontWeight::SEMIBOLD);
        let height = px(CORNER_HEIGHT);
        let label: SharedString = self.display.label.clone().into();
        let run = TextRun {
            len: label.len(),
            font: font.clone(),
            color: tokens.on_accent,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let system = window.text_system().clone();
        let line = system.shape_line(label, font_size, &[run], None);
        let font_id = system.resolve_font(&font);
        let digit_height = system
            .typographic_bounds(font_id, font_size, '0')
            .map(|bounds| bounds.size.height)
            .unwrap_or(font_size * 0.71);
        let baseline_offset = system.baseline_offset(font_id, font_size, height);
        let left = CORNER_PADDING;
        let right = self.display.corner_right_padding;
        let width = f32::from(line.width) + left + right;
        let accent = tokens.accent;
        div()
            .absolute()
            .top(px(CORNER_OFFSET))
            .right(px(CORNER_OFFSET))
            .w(px(width))
            .h(height)
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds: Bounds<Pixels>, _, window, cx| {
                        let scale = window.scale_factor();
                        window.paint_quad(PaintQuad {
                            corner_radii: Corners::all(bounds.size.height / 2.0),
                            ..fill(bounds, accent)
                        });
                        // The digits' vertical middle sits on the badge's middle.
                        let middle = bounds.top() + bounds.size.height / 2.0;
                        let baseline = snap(middle + digit_height / 2.0, scale);
                        let origin_y = baseline - baseline_offset;
                        crate::layout::paint_left_aligned(
                            &line,
                            point(bounds.left() + px(left), origin_y),
                            height,
                            window,
                            cx,
                        );
                    },
                )
                .size_full(),
            )
    }
}

/// The activity pill with its own pulse (R4): a view that reads the shared slow pulse clock
/// (ANM-004) while it paints, so only this view repaints on every pulse step and surrounding
/// lists do not. The pulse opacity ranges over `[0.08, 1]` along the clock's wave.
pub struct ActivityPillView {
    key: SharedString,
    count: usize,
    tokens: BadgeTokens,
}

impl ActivityPillView {
    /// A pill view for `count` running items.
    pub fn new(key: impl Into<SharedString>, count: usize, tokens: BadgeTokens) -> Self {
        Self {
            key: key.into(),
            count,
            tokens,
        }
    }

    /// Updates the count; zero draws nothing.
    pub fn set_count(&mut self, count: usize, cx: &mut vespertine::Context<Self>) {
        if self.count != count {
            self.count = count;
            cx.notify();
        }
    }
}

impl vespertine::Render for ActivityPillView {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut vespertine::Context<Self>,
    ) -> impl IntoElement {
        let key = ElementId::Name(self.key.clone());
        if self.count == 0 {
            return div().id(key).into_any_element();
        }
        let phase = window.animation_phase_slow(cx, vespertine::ACTIVITY_INDICATOR_PERIOD);
        let pulse = 0.08 + 0.92 * vespertine::animation_wave(phase);
        match activity_pill(key.clone(), self.count, pulse, &self.tokens) {
            Some(pill) => pill.into_any_element(),
            None => div().id(key).into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_exact_up_to_the_cap_then_read_99_plus() {
        assert_eq!(count_label(0), "0");
        assert_eq!(count_label(7), "7");
        assert_eq!(count_label(99), "99");
        assert_eq!(count_label(100), "99+");
        assert_eq!(count_label(12345), "99+");
        assert_eq!(count_label_capped(10, 9), "9+");
        assert_eq!(count_label_capped(9, 9), "9");
    }

    #[test]
    fn right_padding_shrinks_by_one_pixel_only_when_capped() {
        let exact = count_display(99, COUNT_CAP);
        assert!(!exact.capped);
        assert_eq!(exact.pill_right_padding, 6.0);
        assert_eq!(exact.corner_right_padding, 3.0);
        let capped = count_display(100, COUNT_CAP);
        assert!(capped.capped);
        assert_eq!(capped.pill_right_padding, 5.0);
        assert_eq!(capped.corner_right_padding, 2.0);
        // The stored count is never altered.
        assert_eq!(capped.count, 100);
        assert_eq!(capped.label, "99+");
        assert_eq!(pill_width(10.0, false), 10.0 + 6.0 + 5.0 + 4.0 + 6.0);
        assert_eq!(pill_width(10.0, true), 10.0 + 6.0 + 5.0 + 4.0 + 5.0);
    }

    #[test]
    fn the_dot_pulses_between_04_and_10() {
        assert!((dot_opacity(0.08) - 0.4).abs() < 1e-6);
        assert!((dot_opacity(1.0) - 1.0).abs() < 1e-6);
        assert!((dot_opacity(0.54) - 0.7).abs() < 1e-6);
        assert!(
            (dot_opacity(0.0) - 0.4).abs() < 1e-6,
            "clamped below the clock's range"
        );
        assert!((dot_opacity(2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn zero_draws_no_pill() {
        let tokens = BadgeTokens::neutral();
        assert!(activity_pill("pill", 0, 1.0, &tokens).is_none());
        assert!(activity_pill("pill", 1, 1.0, &tokens).is_some());
    }
}
