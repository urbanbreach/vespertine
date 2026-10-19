//! The changed-glyph rolling label (ANM-002 R13).
//!
//! A keyed [`RollingLabel`] shows its text plainly at rest and on first observation of a key. When
//! the text changes, the shared leading and trailing runs stay put and only the changed middle
//! glyphs roll: old glyphs rise, shrink and fade out, new glyphs rise from below, grow and fade in,
//! staggered left to right, while the label's width glides and the trailing run slides.
//!
//! The geometry lives in [`RollPlan`] (pure, sampled at an elapsed time) and the remembered keys in
//! [`LabelRollMemory`]; the element glues them to text shaping and painting.

use std::{collections::HashMap, hash::Hash, time::Duration};

use vespertine_util::ResultExt;

use crate::{
    App, Bounds, ContentMask, Element, ElementId, FontId, GlobalElementId, GlyphId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Point, Radians, ScaledPixels, ShapedLine,
    SharedString, Size, Style, TextAlign, TransformationMatrix, Window, point, px, size,
};

use super::presets::{
    LABEL_BLUR_EM, LABEL_GLYPH_CURVE, LABEL_REST_SCALE, LABEL_ROLL, LABEL_STAGGER,
    LABEL_TILT_DEGREES, LABEL_TOTAL_FACTOR, LABEL_TRAVEL_EM, SIZE_CURVE, SIZE_TRANSITION,
};

/// The maximum number of remembered label keys.
pub const MAX_REMEMBERED_KEYS: usize = 256;

/// Lengths, in bytes, of the shared leading and trailing runs of two strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SharedRuns {
    /// Bytes of the common prefix.
    pub leading: usize,
    /// Bytes of the common suffix, never overlapping the prefix in either string.
    pub trailing: usize,
}

/// Computes the shared leading and trailing runs of `old` and `new` on whole characters. The
/// runs never overlap each other in either string: `"feature/a"` to `"fix/a"` shares 1 leading and
/// 2 trailing bytes, `"aa"` to `"aaa"` 2 and 0, `"ab"` to `"b"` 0 and 1.
pub fn shared_runs(old: &str, new: &str) -> SharedRuns {
    let leading: usize = old
        .chars()
        .zip(new.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let limit = old.len().min(new.len()) - leading;
    let mut trailing = 0;
    for (a, b) in old[leading..]
        .chars()
        .rev()
        .zip(new[leading..].chars().rev())
    {
        if a != b || trailing + a.len_utf8() > limit {
            break;
        }
        trailing += a.len_utf8();
    }
    SharedRuns { leading, trailing }
}

/// One shaped glyph as the roll geometry sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LabelGlyph {
    /// The font of the glyph's run.
    pub font_id: FontId,
    /// The glyph.
    pub glyph_id: GlyphId,
    /// X of the glyph origin within its own shaped line, in logical pixels.
    pub x: f32,
    /// The glyph's advance in logical pixels.
    pub advance: f32,
    /// Byte index of the glyph in its text.
    pub index: usize,
    /// Whether the glyph is an emoji or other non-tintable glyph.
    pub is_emoji: bool,
}

impl LabelGlyph {
    /// Collects the glyphs of a shaped line.
    pub fn from_line(line: &ShapedLine) -> Vec<LabelGlyph> {
        let mut glyphs: Vec<LabelGlyph> = line
            .runs
            .iter()
            .flat_map(|run| {
                run.glyphs.iter().map(move |glyph| LabelGlyph {
                    font_id: run.font_id,
                    glyph_id: glyph.id,
                    x: f32::from(glyph.position.x),
                    advance: 0.0,
                    index: glyph.index,
                    is_emoji: glyph.is_emoji,
                })
            })
            .collect();
        let width = f32::from(line.width);
        for i in 0..glyphs.len() {
            let end = glyphs.get(i + 1).map_or(width, |next| next.x);
            glyphs[i].advance = (end - glyphs[i].x).max(0.0);
        }
        glyphs
    }
}

/// Which part of the label a frame glyph belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GlyphRole {
    /// Shared leading run: stays put.
    Leading,
    /// Shared trailing run: slides from its old x to its new x.
    Trailing,
    /// Changed glyph of the old text: rises, shrinks and fades out.
    Old,
    /// Changed glyph of the new text: rises in from below, grows and fades in.
    New,
}

/// One glyph to paint at a sampled instant of the roll.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameGlyph {
    /// The glyph.
    pub glyph: LabelGlyph,
    /// Its role.
    pub role: GlyphRole,
    /// X of the glyph origin relative to the label's left edge, in logical pixels.
    pub x: f32,
    /// Vertical displacement in ems (negative is up).
    pub dy_em: f32,
    /// Scale about the glyph's own center.
    pub scale: f32,
    /// Opacity multiplier from 0 to 1.
    pub opacity: f32,
    /// Tilt in degrees (clockwise positive).
    pub tilt_degrees: f32,
    /// Blur sigma in ems.
    pub blur_em: f32,
}

impl FrameGlyph {
    /// Whether the glyph is painted with no transform and no blur (the ordinary glyph path, or a
    /// pure translation at the translated origin).
    pub fn is_plain(&self) -> bool {
        self.scale == 1.0 && self.tilt_degrees == 0.0 && self.blur_em == 0.0
    }
}

/// The label at one instant of the roll.
#[derive(Clone, Debug, PartialEq)]
pub struct RollFrame {
    /// The label's interpolated width in logical pixels.
    pub width: f32,
    /// The glyphs to paint, in paint order.
    pub glyphs: Vec<FrameGlyph>,
}

struct StaggeredGlyph {
    glyph: LabelGlyph,
    delay: f32,
}

/// The pure geometry of one roll between two shaped texts.
pub struct RollPlan {
    leading: Vec<LabelGlyph>,
    trailing: Vec<LabelGlyph>,
    trailing_slide: f32,
    old: Vec<StaggeredGlyph>,
    new: Vec<StaggeredGlyph>,
    old_width: f32,
    new_width: f32,
    duration: f32,
    size_duration: f32,
    total: Duration,
}

fn stagger(glyphs: Vec<LabelGlyph>, duration: f32) -> Vec<StaggeredGlyph> {
    let n = glyphs.len();
    let (min_x, max_x) = glyphs
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), g| {
            (lo.min(g.x), hi.max(g.x))
        });
    let spread = duration * LABEL_STAGGER * (n.saturating_sub(1)) as f32 / n.max(1) as f32;
    glyphs
        .into_iter()
        .map(|glyph| StaggeredGlyph {
            delay: if max_x > min_x {
                spread * (glyph.x - min_x) / (max_x - min_x)
            } else {
                0.0
            },
            glyph,
        })
        .collect()
}

impl RollPlan {
    /// Builds the plan for a roll from `old` text to `new` text.
    ///
    /// Widths are rounded up to whole logical pixels and the roll lasts `duration` times 1.3; the
    /// width and trailing-slide run on the 200 ms size curve (`size_duration`).
    pub fn new(
        old: (&str, &[LabelGlyph], f32),
        new: (&str, &[LabelGlyph], f32),
        duration: Duration,
        size_duration: Duration,
    ) -> Self {
        let (old_text, old_glyphs, old_width) = old;
        let (new_text, new_glyphs, new_width) = new;
        let runs = shared_runs(old_text, new_text);
        let base_duration = duration;
        let duration = duration.as_secs_f32();

        let split = |glyphs: &[LabelGlyph], len: usize| {
            let (mut leading, mut trailing, mut changed) = (vec![], vec![], vec![]);
            for glyph in glyphs {
                if glyph.index < runs.leading {
                    leading.push(*glyph);
                } else if glyph.index >= len - runs.trailing {
                    trailing.push(*glyph);
                } else {
                    changed.push(*glyph);
                }
            }
            (leading, trailing, changed)
        };
        let (_, old_trailing, old_changed) = split(old_glyphs, old_text.len());
        let (leading, trailing, new_changed) = split(new_glyphs, new_text.len());

        let first_x = |glyphs: &[LabelGlyph]| glyphs.iter().map(|g| g.x).reduce(f32::min);
        let trailing_slide = match (first_x(&old_trailing), first_x(&trailing)) {
            (Some(old_x), Some(new_x)) => old_x - new_x,
            _ => 0.0,
        };

        Self {
            leading,
            trailing,
            trailing_slide,
            old: stagger(old_changed, duration),
            new: stagger(new_changed, duration),
            old_width: old_width.ceil(),
            new_width: new_width.ceil(),
            duration,
            total: roll_total_duration(base_duration),
            size_duration: size_duration.as_secs_f32(),
        }
    }

    /// The whole roll's duration: the glyph duration times 1.3.
    pub fn total_duration(&self) -> Duration {
        self.total
    }

    /// The shared glyph progress curve applied to `(elapsed - delay) / duration`, clamped.
    fn glyph_progress(&self, elapsed: f32, delay: f32) -> f32 {
        let raw = if self.duration > 0.0 {
            (elapsed - delay) / self.duration
        } else {
            1.0
        };
        LABEL_GLYPH_CURVE.ease(raw.clamp(0.0, 1.0))
    }

    /// Samples the roll at `elapsed`. Returns `None` once the whole roll (duration times 1.3) is
    /// over: the plain label then replaces it with identical glyph positions.
    pub fn frame(&self, elapsed: Duration) -> Option<RollFrame> {
        if elapsed >= self.total_duration() {
            return None;
        }
        let e = elapsed.as_secs_f32();
        let size_raw = if self.size_duration > 0.0 {
            (e / self.size_duration).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let size_progress = SIZE_CURVE.ease(size_raw);
        let width = self.old_width + (self.new_width - self.old_width) * size_progress;
        let slide = self.trailing_slide * (1.0 - size_progress);

        let mut glyphs = Vec::with_capacity(
            self.leading.len() + self.trailing.len() + self.old.len() + self.new.len(),
        );
        let resting = |glyph: LabelGlyph, role, x| FrameGlyph {
            glyph,
            role,
            x,
            dy_em: 0.0,
            scale: 1.0,
            opacity: 1.0,
            tilt_degrees: 0.0,
            blur_em: 0.0,
        };
        for glyph in &self.leading {
            glyphs.push(resting(*glyph, GlyphRole::Leading, glyph.x));
        }
        for glyph in &self.trailing {
            glyphs.push(resting(*glyph, GlyphRole::Trailing, glyph.x + slide));
        }
        for item in &self.old {
            let p = self.glyph_progress(e, item.delay);
            // Emoji swap at the midpoint of their progress: shown while alpha is at least 1/2.
            if item.glyph.is_emoji {
                if 1.0 - p >= 0.5 {
                    glyphs.push(FrameGlyph {
                        dy_em: -LABEL_TRAVEL_EM * p,
                        ..resting(item.glyph, GlyphRole::Old, item.glyph.x)
                    });
                }
                continue;
            }
            glyphs.push(FrameGlyph {
                glyph: item.glyph,
                role: GlyphRole::Old,
                x: item.glyph.x,
                dy_em: -LABEL_TRAVEL_EM * p,
                scale: 1.0 - (1.0 - LABEL_REST_SCALE) * p,
                opacity: 1.0 - p,
                tilt_degrees: LABEL_TILT_DEGREES * p,
                blur_em: LABEL_BLUR_EM * p,
            });
        }
        for item in &self.new {
            let p = self.glyph_progress(e, item.delay);
            if item.glyph.is_emoji {
                if p >= 0.5 {
                    glyphs.push(FrameGlyph {
                        dy_em: LABEL_TRAVEL_EM * (1.0 - p),
                        ..resting(item.glyph, GlyphRole::New, item.glyph.x)
                    });
                }
                continue;
            }
            let motion = 1.0 - p;
            glyphs.push(FrameGlyph {
                glyph: item.glyph,
                role: GlyphRole::New,
                x: item.glyph.x,
                dy_em: LABEL_TRAVEL_EM * motion,
                scale: LABEL_REST_SCALE + (1.0 - LABEL_REST_SCALE) * p,
                opacity: p,
                tilt_degrees: LABEL_TILT_DEGREES * motion,
                blur_em: LABEL_BLUR_EM * motion,
            });
        }
        Some(RollFrame { width, glyphs })
    }
}

/// The transform of a rolling glyph in device pixels: scale and tilt about the glyph's own center
/// `pivot`, then the vertical displacement `dy`, all multiplied by the scale factor.
pub fn glyph_transformation(
    pivot: Point<Pixels>,
    dy: Pixels,
    scale: f32,
    tilt_degrees: f32,
    scale_factor: f32,
) -> TransformationMatrix {
    let pivot = pivot.scale(scale_factor);
    let dy = dy.scale(scale_factor);
    TransformationMatrix::unit()
        .translate(point(pivot.x, pivot.y + dy))
        .rotate(Radians(tilt_degrees.to_radians()))
        .scale(Size {
            width: scale,
            height: scale,
        })
        .translate(point(ScaledPixels(-pivot.x.0), ScaledPixels(-pivot.y.0)))
}

/// What a label should show, as decided by [`LabelRollMemory::observe`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation {
    /// Show the text plainly.
    Plain,
    /// A roll from `old` is running since `started`.
    Roll {
        /// The previous text.
        old: SharedString,
        /// The time the roll started.
        started: Duration,
    },
}

struct Remembered {
    text: SharedString,
    roll: Option<(SharedString, Duration)>,
}

/// The remembered label keys (at most [`MAX_REMEMBERED_KEYS`]).
pub struct LabelRollMemory<K = ElementId> {
    keys: HashMap<K, Remembered>,
}

impl<K> Default for LabelRollMemory<K> {
    fn default() -> Self {
        Self {
            keys: HashMap::new(),
        }
    }
}

impl<K: Eq + Hash + Clone> LabelRollMemory<K> {
    /// The number of remembered keys.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether no key is remembered.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Whether `key` is remembered.
    pub fn contains(&self, key: &K) -> bool {
        self.keys.contains_key(key)
    }

    /// Observes `text` for `key` at time `now`.
    ///
    /// The first sight of a key shows the text plainly. A change starts a roll (a roll in
    /// progress restarts from the previously shown target). Reduced motion settles immediately
    /// and a finished roll settles. When the memory is full, settled keys are forgotten to make
    /// room; if every key is still rolling, the new key is not remembered.
    pub fn observe(
        &mut self,
        key: &K,
        text: &SharedString,
        now: Duration,
        reduced_motion: bool,
        roll_total: Duration,
    ) -> Observation {
        if let Some(entry) = self.keys.get_mut(key) {
            if reduced_motion {
                entry.text = text.clone();
                entry.roll = None;
                return Observation::Plain;
            }
            if entry.text != *text {
                entry.roll = Some((std::mem::replace(&mut entry.text, text.clone()), now));
            }
            if let Some((_, started)) = &entry.roll
                && now.saturating_sub(*started) >= roll_total
            {
                entry.roll = None;
            }
            return match &entry.roll {
                Some((old, started)) => Observation::Roll {
                    old: old.clone(),
                    started: *started,
                },
                None => Observation::Plain,
            };
        }

        if self.keys.len() >= MAX_REMEMBERED_KEYS {
            self.keys.retain(|_, entry| {
                entry
                    .roll
                    .as_ref()
                    .is_some_and(|(_, started)| now.saturating_sub(*started) < roll_total)
            });
        }
        if self.keys.len() < MAX_REMEMBERED_KEYS {
            self.keys.insert(
                key.clone(),
                Remembered {
                    text: text.clone(),
                    roll: None,
                },
            );
        }
        Observation::Plain
    }
}

impl crate::Global for LabelRollMemory<ElementId> {}

/// The label roll's duration (550 ms scaled by the global timeline scale).
pub fn roll_duration() -> Duration {
    LABEL_ROLL.duration()
}

/// The whole roll's duration for a glyph duration: that duration times 1.3 (715 ms for 550 ms).
pub fn roll_total_duration(duration: Duration) -> Duration {
    Duration::from_secs_f64(duration.as_secs_f64() * LABEL_TOTAL_FACTOR)
}

/// The size-curve duration (200 ms scaled by the global timeline scale).
pub fn size_duration() -> Duration {
    SIZE_TRANSITION.duration()
}

/// The roll plan's content mask: the label bounds expanded vertically by twice the travel and
/// horizontally by the blur halo (three times the blur radius, rounded up).
pub fn roll_mask_bounds(bounds: Bounds<Pixels>, font_size: Pixels) -> Bounds<Pixels> {
    let travel = font_size * LABEL_TRAVEL_EM;
    let halo = px((3.0 * f32::from(font_size) * LABEL_BLUR_EM).ceil());
    Bounds {
        origin: point(bounds.origin.x - halo, bounds.origin.y - travel * 2.0),
        size: size(
            bounds.size.width + halo * 2.0,
            bounds.size.height + travel * 4.0,
        ),
    }
}

/// Everything needed to paint one rolling glyph. This is the single place where glyph painting
/// meets the renderer: tilt, scale and blur are composite-time operations of RND-005's transformed
/// glyph painter.
pub struct RollGlyphPaint {
    /// Baseline origin of the glyph at rest, in window logical pixels.
    pub origin: Point<Pixels>,
    /// The glyph's font.
    pub font_id: FontId,
    /// The glyph.
    pub glyph_id: GlyphId,
    /// The font size.
    pub font_size: Pixels,
    /// Foreground color.
    pub color: crate::Hsla,
    /// Opacity multiplier.
    pub opacity: f32,
    /// Transform in device pixels about the glyph's own center, including the vertical rise.
    pub transformation: TransformationMatrix,
    /// Blur sigma in logical pixels.
    pub blur: Pixels,
    /// Whether the glyph is an emoji (translation only, ordinary color-glyph path).
    pub is_emoji: bool,
}

/// Paints one rolling glyph.
///
/// Emoji, and glyphs whose transform is a pure translation with no blur, use the ordinary glyph
/// path at the translated origin (emoji only ever translate). Glyphs with tilt, scale or blur use
/// RND-005's composite-time transformed glyph painter: the same cached raster tile is sampled with
/// the transform (in device pixels, pivot included) and the blur sigma, so rolling never
/// rasterizes or uploads anything new.
pub fn paint_roll_glyph(window: &mut Window, paint: &RollGlyphPaint) -> crate::Result<()> {
    let scale_factor = window.scale_factor();
    let [[xx, xy], [yx, yy]] = paint.transformation.rotation_scale;
    let translation_only = xx == 1.0 && xy == 0.0 && yx == 0.0 && yy == 1.0;
    window.with_element_opacity(Some(paint.opacity), |window| {
        if !paint.is_emoji && (!translation_only || paint.blur > px(0.0)) {
            return window.paint_glyph_transformed(
                paint.origin,
                paint.font_id,
                paint.glyph_id,
                paint.font_size,
                paint.color,
                paint.transformation,
                paint.blur,
            );
        }
        let moved = paint
            .transformation
            .apply(paint.origin.scale(scale_factor).map(|v| px(v.0)));
        let origin = point(moved.x / scale_factor, moved.y / scale_factor);
        if paint.is_emoji {
            window.paint_emoji(origin, paint.font_id, paint.glyph_id, paint.font_size)
        } else {
            window.paint_glyph(
                origin,
                paint.font_id,
                paint.glyph_id,
                paint.font_size,
                paint.color,
            )
        }
    })
}

/// A keyed label that rolls its changed glyphs when its text changes. Identity is the `key`;
/// the style (font, size, color, line height) comes from the surrounding text style.
pub struct RollingLabel {
    key: ElementId,
    text: SharedString,
}

impl RollingLabel {
    /// Creates a label identified by `key` showing `text`.
    pub fn new(key: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            text: text.into(),
        }
    }
}

impl IntoElement for RollingLabel {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Layout state of a [`RollingLabel`].
pub struct RollingLabelLayout {
    line: ShapedLine,
    line_height: Pixels,
    font_size: Pixels,
    color: crate::Hsla,
    roll: Option<(RollPlan, ShapedLine, RollFrame)>,
}

impl Element for RollingLabel {
    type RequestLayoutState = RollingLabelLayout;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = window.text_style();
        let rem_size = window.rem_size();
        let font_size = style.font_size.to_pixels(rem_size);
        let line_height = style.line_height_in_pixels(rem_size);
        let run = style.to_run(self.text.len());
        let color = style.color;
        let text_system = window.text_system().clone();
        let line = text_system.shape_line(self.text.clone(), font_size, &[run], None);

        let now = super::now(cx);
        let reduced = cx.reduce_motion();
        let total = roll_total_duration(roll_duration());
        let observation = cx
            .default_global::<LabelRollMemory>()
            .observe(&self.key, &self.text, now, reduced, total);

        let mut roll = None;
        let mut width = f32::from(line.width);
        if let Observation::Roll { old, started } = observation {
            let old_run = style.to_run(old.len());
            let old_line = text_system.shape_line(old, font_size, &[old_run], None);
            let plan = RollPlan::new(
                (
                    &old_line.text,
                    &LabelGlyph::from_line(&old_line),
                    f32::from(old_line.width),
                ),
                (
                    &line.text,
                    &LabelGlyph::from_line(&line),
                    f32::from(line.width),
                ),
                roll_duration(),
                size_duration(),
            );
            if let Some(frame) = plan.frame(now.saturating_sub(started)) {
                width = frame.width;
                roll = Some((plan, old_line, frame));
            }
        }

        let layout_style = Style {
            size: Size {
                width: px(width).into(),
                height: line_height.into(),
            },
            ..Style::default()
        };
        let layout_id = window.request_layout(layout_style, [], cx);
        (
            layout_id,
            RollingLabelLayout {
                line,
                line_height,
                font_size,
                color,
                roll,
            },
        )
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((_, _, frame)) = &state.roll else {
            let (origin, line_height) = (bounds.origin, state.line_height);
            state
                .line
                .paint(origin, line_height, TextAlign::Left, None, window, cx)
                .log_err();
            return;
        };

        // The roll draws its frames from the shared bounded clock while it is painting.
        window.renew_animation_lease(cx, 1);

        let layout = &state.line;
        let padding_top = (state.line_height - layout.ascent - layout.descent) / 2.0;
        let baseline = bounds.origin.y + padding_top + layout.ascent;
        let scale_factor = window.scale_factor();
        let mask = ContentMask {
            bounds: roll_mask_bounds(bounds, state.font_size),
        };
        window.with_content_mask(Some(mask), |window| {
            for glyph in &frame.glyphs {
                if glyph.opacity <= 0.0 {
                    continue;
                }
                let origin = point(bounds.origin.x + px(glyph.x), baseline);
                let dy = state.font_size * glyph.dy_em;
                let pivot = point(
                    origin.x + px(glyph.glyph.advance / 2.0),
                    baseline - (layout.ascent - layout.descent) / 2.0,
                );
                let paint = RollGlyphPaint {
                    origin,
                    font_id: glyph.glyph.font_id,
                    glyph_id: glyph.glyph.glyph_id,
                    font_size: state.font_size,
                    color: state.color,
                    opacity: glyph.opacity,
                    transformation: glyph_transformation(
                        pivot,
                        dy,
                        glyph.scale,
                        glyph.tilt_degrees,
                        scale_factor,
                    ),
                    blur: state.font_size * glyph.blur_em,
                    is_emoji: glyph.glyph.is_emoji,
                };
                paint_roll_glyph(window, &paint).log_err();
            }
        });
    }
}
