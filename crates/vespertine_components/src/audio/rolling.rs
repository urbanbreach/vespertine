//! The keyed rolling-label transition (CMP-027 R17-R20): a text value that animates only its
//! changed middle when the value for a key changes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use vespertine::{
    AnyElement, App, Bounds, FontId, GlyphId, Hsla, IntoElement, ParentElement, Pixels, Point,
    Radians, ScaledPixels, ShapedLine, SharedString, Styled, TextRun, TransformationMatrix, Window,
    canvas, div, point, px, size,
};

/// Duration of one glyph's roll.
pub const ROLL_DURATION: Duration = Duration::from_millis(550);
/// Per-glyph travel, in em.
pub const TRAVEL_EM: f32 = 0.35;
/// Scale of a glyph at rest in the off state.
pub const REST_SCALE: f32 = 0.6;
/// Tilt at full strength, in degrees.
pub const TILT_DEGREES: f32 = 2.0;
/// Blur radius at full strength, in em.
pub const BLUR_EM: f32 = 0.1;
/// Stagger as a fraction of the duration.
pub const STAGGER: f32 = 0.3;
/// The width and sliding-suffix curve duration.
pub const RESIZE_DURATION: Duration = Duration::from_millis(200);
/// Most keys remembered per thread.
pub const KEY_CAP: usize = 256;

/// The longest common leading run and then the longest common trailing run of `old` and `new`,
/// in characters, never overlapping in either string (R18).
pub fn shared_ends(old: &str, new: &str) -> (usize, usize) {
    let old: Vec<char> = old.chars().collect();
    let new: Vec<char> = new.chars().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let limit = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(limit)
        .take_while(|(a, b)| a == b)
        .count();
    (prefix, suffix)
}

/// A cubic Bezier easing `(x1, y1, x2, y2)` evaluated at `t` in `[0, 1]` (solved by bisection
/// on x, so it is monotone whenever the control abscissae are in `[0, 1]`).
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let coordinate = |a: f32, b: f32, u: f32| {
        let v = 1.0 - u;
        3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if coordinate(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    coordinate(y1, y2, (lo + hi) / 2.0)
}

/// The glyph curve: `cubic-bezier(0.22, 1, 0.36, 1)` (long settle, no overshoot).
pub fn glide(t: f32) -> f32 {
    cubic_bezier(0.22, 1.0, 0.36, 1.0, t)
}

/// The 200 ms resize curve (ease-out).
pub fn resize_curve(t: f32) -> f32 {
    cubic_bezier(0.0, 0.0, 0.58, 1.0, t)
}

/// Total duration of a roll: `550 ms x (1 + 0.3)` = 715 ms, divided by the global motion speed.
pub fn total_duration(speed: f32) -> Duration {
    let speed = if speed.is_finite() && speed > 0.0 {
        speed
    } else {
        1.0
    };
    Duration::from_micros(
        (ROLL_DURATION.as_micros() as f64 * f64::from(1.0 + STAGGER) / f64::from(speed)).round()
            as u64,
    )
}

/// Start delay of a changed glyph at `x` among `n` changed glyphs spanning `[x_min, x_max]`:
/// `spread x clamp((x - x_min) / (x_max - x_min), 0, 1)` with
/// `spread = 550 ms x 0.3 x (n - 1) / n`; 0 when `x_max <= x_min`.
pub fn glyph_delay(x: f32, x_min: f32, x_max: f32, n: usize) -> Duration {
    if n == 0 || x_max <= x_min {
        return Duration::ZERO;
    }
    let spread = ROLL_DURATION.as_secs_f32() * STAGGER * (n - 1) as f32 / n as f32;
    Duration::from_secs_f32(spread * ((x - x_min) / (x_max - x_min)).clamp(0.0, 1.0))
}

/// How a changed glyph is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Vertical offset in em, positive down.
    pub dy_em: f32,
    /// Uniform scale about the glyph center.
    pub scale: f32,
    /// Opacity.
    pub alpha: f32,
    /// Tilt/blur strength in `[0, 1]`.
    pub strength: f32,
}

impl Pose {
    /// Tilt angle in degrees: `2 x strength`.
    pub fn tilt_degrees(&self) -> f32 {
        TILT_DEGREES * self.strength
    }

    /// Blur radius in em: `0.1 x strength`.
    pub fn blur_em(&self) -> f32 {
        BLUR_EM * self.strength
    }
}

/// An outgoing glyph at glide progress `p`: up by `0.35 em x p`, scale `lerp(1, 0.6, p)`,
/// alpha `1 - p`, strength `p`.
pub fn outgoing_pose(p: f32) -> Pose {
    Pose {
        dy_em: -TRAVEL_EM * p,
        scale: 1.0 + (REST_SCALE - 1.0) * p,
        alpha: 1.0 - p,
        strength: p,
    }
}

/// An incoming glyph: `0.35 em x (1 - p)` below rest, scale `lerp(0.6, 1, p)`, alpha `p`,
/// strength `1 - p`.
pub fn incoming_pose(p: f32) -> Pose {
    Pose {
        dy_em: TRAVEL_EM * (1.0 - p),
        scale: REST_SCALE + (1.0 - REST_SCALE) * p,
        alpha: p,
        strength: 1.0 - p,
    }
}

/// Glide progress of a glyph with start `delay` at `elapsed`.
pub fn glyph_progress(elapsed: Duration, delay: Duration) -> f32 {
    let local = elapsed.saturating_sub(delay).as_secs_f32() / ROLL_DURATION.as_secs_f32();
    glide(local.clamp(0.0, 1.0))
}

/// Layout progress (label width and sliding suffix) after `elapsed`: the 200 ms ease-out.
pub fn layout_progress(elapsed: Duration) -> f32 {
    resize_curve(elapsed.as_secs_f32() / RESIZE_DURATION.as_secs_f32())
}

#[derive(Clone)]
struct ActiveRoll {
    old: SharedString,
    new: SharedString,
    start: Instant,
}

struct Entry {
    text: SharedString,
    roll: Option<ActiveRoll>,
}

thread_local! {
    static ENTRIES: RefCell<HashMap<SharedString, Entry>> = RefCell::new(HashMap::new());
}

/// A running roll.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Roll {
    /// The previous text.
    pub old: SharedString,
    /// The new text.
    pub new: SharedString,
    /// Time since the roll started.
    pub elapsed: Duration,
}

/// Observes `text` for `key` at `now` (R17): the first value for a key never animates; a change
/// starts a roll; reduced motion ends any roll at once; remembered keys are bounded to
/// [`KEY_CAP`] per thread (settled entries are forgotten when full). Returns the roll while one
/// is active.
pub fn observe(
    key: &SharedString,
    text: &SharedString,
    reduced: bool,
    now: Instant,
) -> Option<Roll> {
    ENTRIES.with(|entries| {
        let mut entries = entries.borrow_mut();
        if !entries.contains_key(key) {
            if entries.len() >= KEY_CAP {
                entries.retain(|_, entry| entry.roll.is_some());
            }
            if entries.len() < KEY_CAP {
                entries.insert(
                    key.clone(),
                    Entry {
                        text: text.clone(),
                        roll: None,
                    },
                );
            }
            return None;
        }
        let entry = entries.get_mut(key)?;
        if reduced {
            entry.text = text.clone();
            entry.roll = None;
            return None;
        }
        if entry.text != *text {
            entry.roll = Some(ActiveRoll {
                old: entry.text.clone(),
                new: text.clone(),
                start: now,
            });
            entry.text = text.clone();
        }
        let roll = entry.roll.clone()?;
        let elapsed = now.saturating_duration_since(roll.start);
        if elapsed >= total_duration(1.0) {
            entry.roll = None;
            return None;
        }
        Some(Roll {
            old: roll.old,
            new: roll.new,
            elapsed,
        })
    })
}

/// Forgets every remembered key of this thread.
pub fn forget_all() {
    ENTRIES.with(|entries| entries.borrow_mut().clear());
}

/// Number of remembered keys on this thread.
pub fn remembered_keys() -> usize {
    ENTRIES.with(|entries| entries.borrow().len())
}

/// One glyph to paint.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphPlacement {
    /// Font.
    pub font_id: FontId,
    /// Glyph.
    pub glyph_id: GlyphId,
    /// Whether the glyph is an emoji (cannot be transformed).
    pub is_emoji: bool,
    /// Baseline-left x relative to the label's left edge.
    pub x: f32,
    /// Advance (distance to the next glyph, or to the end of the line).
    pub advance: f32,
    /// How it is drawn; `None` at rest.
    pub pose: Option<Pose>,
}

/// Glyphs of a shaped line with their character index.
fn glyphs_of(line: &ShapedLine) -> Vec<(usize, FontId, GlyphId, bool, f32)> {
    let text = line.text.as_ref();
    let char_of = |byte: usize| text[..byte.min(text.len())].chars().count();
    let mut glyphs = Vec::new();
    for run in &line.runs {
        for glyph in &run.glyphs {
            glyphs.push((
                char_of(glyph.index),
                run.font_id,
                glyph.id,
                glyph.is_emoji,
                f32::from(glyph.position.x),
            ));
        }
    }
    glyphs
}

/// Places every glyph of a roll at `elapsed` (R18, R19). Returns the placements and the label
/// width. `old_line` and `new_line` are the shaped lines of the old and new text.
pub fn place_glyphs(
    old_line: &ShapedLine,
    new_line: &ShapedLine,
    elapsed: Duration,
) -> (Vec<GlyphPlacement>, f32) {
    let (prefix, suffix) = shared_ends(&old_line.text, &new_line.text);
    let old_glyphs = glyphs_of(old_line);
    let new_glyphs = glyphs_of(new_line);
    let old_chars = old_line.text.chars().count();
    let new_chars = new_line.text.chars().count();
    let old_width = f32::from(old_line.width());
    let new_width = f32::from(new_line.width());
    let layout = layout_progress(elapsed);
    let width = (old_width + (new_width - old_width) * layout).ceil();

    let advance_of = |glyphs: &[(usize, FontId, GlyphId, bool, f32)], index: usize, end: f32| {
        glyphs.get(index + 1).map_or(end, |next| next.4) - glyphs[index].4
    };
    let suffix_x = |glyphs: &[(usize, FontId, GlyphId, bool, f32)], chars: usize, end: f32| {
        glyphs
            .iter()
            .find(|glyph| glyph.0 >= chars - suffix)
            .map_or(end, |glyph| glyph.4)
    };
    let old_suffix_x = suffix_x(&old_glyphs, old_chars, old_width);
    let new_suffix_x = suffix_x(&new_glyphs, new_chars, new_width);
    let slide = (new_suffix_x - old_suffix_x) * (1.0 - layout);

    let mut placements = Vec::new();
    let middle = |chars: usize, glyph: usize| glyph >= prefix && glyph < chars - suffix;
    // Outgoing and incoming changed glyphs, each side staggered left to right.
    let side = |glyphs: &[(usize, FontId, GlyphId, bool, f32)], chars: usize| {
        let xs: Vec<f32> = glyphs
            .iter()
            .filter(|glyph| middle(chars, glyph.0))
            .map(|glyph| glyph.4)
            .collect();
        let min = xs.iter().copied().fold(f32::INFINITY, f32::min);
        let max = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        (xs.len(), min, max)
    };
    let (old_n, old_min, old_max) = side(&old_glyphs, old_chars);
    let (new_n, new_min, new_max) = side(&new_glyphs, new_chars);

    for (index, glyph) in new_glyphs.iter().enumerate() {
        let advance = advance_of(&new_glyphs, index, new_width);
        if glyph.0 < prefix {
            placements.push(GlyphPlacement {
                font_id: glyph.1,
                glyph_id: glyph.2,
                is_emoji: glyph.3,
                x: glyph.4,
                advance,
                pose: None,
            });
        } else if glyph.0 >= new_chars - suffix {
            placements.push(GlyphPlacement {
                font_id: glyph.1,
                glyph_id: glyph.2,
                is_emoji: glyph.3,
                x: glyph.4 - slide,
                advance,
                pose: None,
            });
        } else {
            let delay = glyph_delay(glyph.4, new_min, new_max, new_n);
            placements.push(GlyphPlacement {
                font_id: glyph.1,
                glyph_id: glyph.2,
                is_emoji: glyph.3,
                x: glyph.4,
                advance,
                pose: Some(incoming_pose(glyph_progress(elapsed, delay))),
            });
        }
    }
    for (index, glyph) in old_glyphs.iter().enumerate() {
        if !middle(old_chars, glyph.0) {
            continue;
        }
        let delay = glyph_delay(glyph.4, old_min, old_max, old_n);
        placements.push(GlyphPlacement {
            font_id: glyph.1,
            glyph_id: glyph.2,
            is_emoji: glyph.3,
            x: glyph.4,
            advance: advance_of(&old_glyphs, index, old_width),
            pose: Some(outgoing_pose(glyph_progress(elapsed, delay))),
        });
    }
    (placements, width)
}

fn shape(window: &Window, text: &SharedString, font_size: Pixels, color: Hsla) -> ShapedLine {
    let font = window.text_style().font();
    let run = TextRun {
        font,
        len: text.len(),
        strikethrough: None,
        underline: None,
        background_color: None,
        color,
    };
    let system = window.text_system();
    system.shape_line(text.clone(), font_size, &[run], None)
}

/// The element of a running roll.
fn roll_element(roll: Roll, window: &Window) -> AnyElement {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let line_height = style.line_height_in_pixels(window.rem_size());
    let color = style.color;
    let old_line = shape(window, &roll.old, font_size, color);
    let new_line = shape(window, &roll.new, font_size, color);
    let (placements, width) = place_glyphs(&old_line, &new_line, roll.elapsed);
    let ascent = f32::from(new_line.ascent);
    let descent = f32::from(new_line.descent);
    let em = f32::from(font_size);
    let line_height = f32::from(line_height);
    let baseline = (line_height - (ascent + descent)) / 2.0 + ascent;
    // Painting is clipped to the label bounds extended by 2 x travel vertically and by
    // ceil(3 x blur) horizontally.
    let extend_y = 2.0 * TRAVEL_EM * em;
    let extend_x = (3.0 * BLUR_EM * em).ceil();
    let (emoji, plain): (Vec<_>, Vec<_>) = placements
        .into_iter()
        .partition(|p| p.is_emoji && p.pose.is_some());

    let paint_plain = canvas(
        |_, _, _| (),
        move |bounds: Bounds<Pixels>, _, window: &mut Window, cx| {
            // The shared ~30 Hz animation clock, only while a roll is active.
            window.renew_animation_lease(cx, 1);
            let mask = vespertine::ContentMask {
                bounds: Bounds {
                    origin: point(
                        bounds.origin.x - px(extend_x),
                        bounds.origin.y - px(extend_y),
                    ),
                    size: size(
                        bounds.size.width + px(2.0 * extend_x),
                        bounds.size.height + px(2.0 * extend_y),
                    ),
                },
            };
            window.with_content_mask(Some(mask), |window| {
                for glyph in &plain {
                    paint_glyph(window, bounds.origin, baseline, em, color, font_size, glyph);
                }
            });
        },
    )
    .absolute()
    .size_full();

    let mut root = div()
        .relative()
        .flex_none()
        .w(px(width))
        .h(px(line_height))
        .child(paint_plain);
    for glyph in emoji {
        // Emoji cannot be transformed: they swap at alpha 0.5 and ride the translation only.
        let (dy, font_id, glyph_id, x) = (
            glyph.pose.map_or(0.0, |pose| pose.dy_em * em),
            glyph.font_id,
            glyph.glyph_id,
            glyph.x,
        );
        root = root.child(
            div().absolute().size_full().opacity(0.5).child(
                canvas(
                    |_, _, _| (),
                    move |bounds: Bounds<Pixels>, _, window: &mut Window, _| {
                        window
                            .paint_emoji(
                                point(bounds.origin.x + px(x), bounds.origin.y + px(baseline + dy)),
                                font_id,
                                glyph_id,
                                font_size,
                            )
                            .ok();
                    },
                )
                .size_full(),
            ),
        );
    }
    root.into_any_element()
}

fn paint_glyph(
    window: &mut Window,
    origin: Point<Pixels>,
    baseline: f32,
    em: f32,
    color: Hsla,
    font_size: Pixels,
    glyph: &GlyphPlacement,
) {
    let at = point(origin.x + px(glyph.x), origin.y + px(baseline));
    let Some(pose) = glyph.pose else {
        window
            .paint_glyph(at, glyph.font_id, glyph.glyph_id, font_size, color)
            .ok();
        return;
    };
    let mut color = color;
    color.a *= pose.alpha;
    if color.a <= 0.0 {
        return;
    }
    let scale_factor = window.scale_factor();
    // Pivot: the glyph center, in device pixels.
    let center = point(
        (at.x + px(glyph.advance / 2.0)) * scale_factor,
        (origin.y + px(baseline - 0.3 * em)) * scale_factor,
    );
    let shift = px(pose.dy_em * em) * scale_factor;
    let to = |x: Pixels, y: Pixels| point(ScaledPixels(f32::from(x)), ScaledPixels(f32::from(y)));
    let matrix = TransformationMatrix::unit()
        .translate(to(center.x, center.y + shift))
        .rotate(Radians(pose.tilt_degrees().to_radians()))
        .scale(size(pose.scale, pose.scale))
        .translate(to(-center.x, -center.y));
    window
        .paint_glyph_transformed(
            at,
            glyph.font_id,
            glyph.glyph_id,
            font_size,
            color,
            matrix,
            px(pose.blur_em() * em),
        )
        .ok();
}

/// The roll element, present only while a roll is active; for callers with their own at-rest
/// presentation. The text style of the surrounding element is used.
pub fn rolling(
    window: &Window,
    cx: &App,
    key: impl Into<SharedString>,
    text: impl Into<SharedString>,
    reduced: bool,
) -> Option<AnyElement> {
    let roll = observe(
        &key.into(),
        &text.into(),
        reduced || cx.reduce_motion(),
        cx.background_executor().now(),
    )?;
    Some(roll_element(roll, window))
}

/// A label whose value rolls when it changes for `key`: the rolling element mid-roll, otherwise
/// an ordinary truncating label (layout, ellipsis and hit testing unchanged).
pub fn roll_text(
    window: &Window,
    cx: &App,
    key: impl Into<SharedString>,
    text: impl Into<SharedString>,
    reduced: bool,
) -> AnyElement {
    let text = text.into();
    match rolling(window, cx, key, text.clone(), reduced) {
        Some(element) => element,
        None => div()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(text)
            .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a10_shared_ends() {
        let cases = [
            ("Opus 5.5", "Opus 5.1", (7, 0)),
            ("feature/a", "fix/a", (1, 2)),
            ("main", "main-2", (4, 0)),
            ("aa", "aaa", (2, 0)),
            ("ab", "b", (0, 1)),
            ("", "x", (0, 0)),
            ("same", "same", (4, 0)),
        ];
        for (old, new, expected) in cases {
            assert_eq!(shared_ends(old, new), expected, "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn a10_glide_is_monotone_without_overshoot() {
        let mut previous = glide(0.0);
        assert!(previous.abs() < 1e-4);
        for step in 1..=200 {
            let value = glide(step as f32 / 200.0);
            assert!(value >= previous - 1e-6, "{step}");
            assert!(value <= 1.0 + 1e-6);
            previous = value;
        }
        assert!((glide(1.0) - 1.0).abs() < 1e-4);
        assert!(resize_curve(0.5) > 0.5);
    }

    #[test]
    fn timing_and_poses_follow_the_spec() {
        assert_eq!(total_duration(1.0), Duration::from_millis(715));
        assert_eq!(total_duration(2.0).as_micros(), 357_500);
        // n = 4 changed glyphs: spread = 550 ms x 0.3 x 3 / 4 = 123.75 ms.
        let delay = glyph_delay(10.0, 0.0, 10.0, 4);
        assert!((delay.as_secs_f32() - 0.12375).abs() < 1e-5);
        assert_eq!(glyph_delay(0.0, 0.0, 10.0, 4), Duration::ZERO);
        assert_eq!(glyph_delay(5.0, 5.0, 5.0, 3), Duration::ZERO);
        let out = outgoing_pose(0.5);
        assert_eq!((out.dy_em, out.alpha, out.strength), (-0.175, 0.5, 0.5));
        assert!((out.scale - 0.8).abs() < 1e-6);
        let incoming = incoming_pose(0.25);
        assert!((incoming.dy_em - 0.2625).abs() < 1e-6);
        assert!((incoming.scale - 0.7).abs() < 1e-6);
        assert!((incoming.tilt_degrees() - 1.5).abs() < 1e-6);
        assert!((incoming.blur_em() - 0.075).abs() < 1e-6);
        assert_eq!(
            glyph_progress(Duration::ZERO, Duration::from_millis(100)),
            0.0
        );
        assert!((glyph_progress(Duration::from_secs(5), Duration::ZERO) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn a10_first_observation_never_rolls_and_reduced_motion_ends_a_roll() {
        forget_all();
        let now = Instant::now();
        let key: SharedString = "model".into();
        assert!(observe(&key, &"Opus 5.5".into(), false, now).is_none());
        let roll = observe(&key, &"Opus 5.1".into(), false, now).unwrap();
        assert_eq!(
            (roll.old.as_ref(), roll.new.as_ref()),
            ("Opus 5.5", "Opus 5.1")
        );
        let mid = observe(
            &key,
            &"Opus 5.1".into(),
            false,
            now + Duration::from_millis(300),
        );
        assert_eq!(mid.unwrap().elapsed, Duration::from_millis(300));
        // Reduced motion switched on mid-roll ends the roll at once.
        assert!(
            observe(
                &key,
                &"Opus 5.1".into(),
                true,
                now + Duration::from_millis(310)
            )
            .is_none()
        );
        assert!(
            observe(
                &key,
                &"Opus 5.1".into(),
                false,
                now + Duration::from_millis(320)
            )
            .is_none()
        );
        // The roll settles after the total duration.
        observe(&key, &"x".into(), false, now + Duration::from_secs(1)).unwrap();
        assert!(observe(&key, &"x".into(), false, now + Duration::from_secs(2)).is_none());
    }

    #[test]
    fn remembered_keys_are_bounded_and_settled_entries_are_forgotten() {
        forget_all();
        let now = Instant::now();
        for i in 0..KEY_CAP {
            observe(&format!("k{i}").into(), &"a".into(), false, now);
        }
        assert_eq!(remembered_keys(), KEY_CAP);
        // One key is mid-roll; the rest are settled and are forgotten when a new key arrives.
        observe(&"k0".into(), &"b".into(), false, now);
        observe(&"fresh".into(), &"a".into(), false, now);
        assert_eq!(remembered_keys(), 2);
        forget_all();
    }
}
