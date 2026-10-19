//! The animated-geometry component (CMP-027 R12-R16): paints externally generated line and dot
//! frames on a caller-controlled clock with strict start/stop rules.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use vespertine::{
    App, Bounds, Context, Corners, Hsla, IntoElement, LineCap, LineJoin, ParentElement,
    PathBuilder, PathStyle, Pixels, Render, SharedString, StrokeOptions, Styled, Subscription,
    Task, Window, canvas, div, fill, hsla, point, px, size,
};
use vespertine_theme::appearance;

/// Reduced-motion convention of the host-ticked variant: the static animation time.
pub const STATIC_TIME: f32 = 0.6;
/// Primitives with an alpha below this are skipped.
pub const ALPHA_CULL: f32 = 0.02;
/// The default target frame rate.
pub const DEFAULT_FPS: f32 = 30.0;
/// Smallest accepted uniform magnification.
pub const MIN_SCALE: f32 = 0.25;
/// Largest accepted uniform magnification.
pub const MAX_SCALE: f32 = 8.0;

/// A line of a frame, in the producer's coordinate space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Line {
    /// Start x.
    pub x1: f32,
    /// Start y.
    pub y1: f32,
    /// End x.
    pub x2: f32,
    /// End y.
    pub y2: f32,
    /// Stroke width.
    pub width: f32,
    /// Ink value in `[0, 1]`.
    pub ink: f32,
    /// Alpha in `[0, 1]`.
    pub alpha: f32,
}

/// A dot of a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dot {
    /// Center x.
    pub x: f32,
    /// Center y.
    pub y: f32,
    /// Radius.
    pub radius: f32,
    /// Ink value in `[0, 1]`.
    pub ink: f32,
    /// Alpha in `[0, 1]`.
    pub alpha: f32,
}

/// One frame: two reusable lists. The producer clears and refills it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    /// Lines, painted first, in order.
    pub lines: Vec<Line>,
    /// Dots, painted after all lines, in order (back to front as emitted).
    pub dots: Vec<Dot>,
}

impl Frame {
    /// An empty frame.
    pub fn new() -> Self {
        Self::default()
    }

    /// Empties both lists, keeping their capacity.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.dots.clear();
    }
}

/// What a producer is asked to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameRequest {
    /// The producer's state (for example idle, listening, speaking).
    pub state: SharedString,
    /// The producer's size preset.
    pub size: SharedString,
    /// Animation time in seconds on the component's animation clock (already multiplied by
    /// the speed). Unbounded.
    pub time: f32,
    /// Duration of state crossfades.
    pub transition: Duration,
    /// Microphone level in `[0, 1]`; influences only motion.
    pub mic_level: f32,
    /// Speaker level in `[0, 1]`; influences only motion.
    pub speaker_level: f32,
    /// Whether the frame is a static reduced-motion frame.
    pub reduced_motion: bool,
}

/// The producer of frames. `generate` is the only place the producer's math runs; the
/// component calls it only when the retained frame is dirty.
pub trait FrameSource: 'static {
    /// Clears `frame` and fills it for `request`.
    fn generate(&mut self, request: &FrameRequest, frame: &mut Frame);

    /// The animation clock stopped (hidden, paused, reduced motion, inactive window): drop any
    /// running transition; state switches now happen without a crossfade.
    fn clock_stopped(&mut self) {}
}

/// The ink direction of the grayscale mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InkTheme {
    /// Lightness `w` (ink on light surfaces).
    Light,
    /// Lightness `1 - w` (ink on dark surfaces).
    Dark,
    /// Follow the theme appearance.
    #[default]
    Auto,
}

/// A target frame rate sanitized to `[1, 30]`; non-finite values become 30.
pub fn sanitize_fps(fps: f32) -> f32 {
    if fps.is_finite() {
        fps.clamp(1.0, DEFAULT_FPS)
    } else {
        DEFAULT_FPS
    }
}

/// A scale sanitized to `[0.25, 8]`; non-finite values become 1.
pub fn sanitize_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        1.0
    }
}

/// The grayscale ink mapping: lightness `w` on light themes and `1 - w` on dark themes.
pub fn ink_color(ink: f32, alpha: f32, dark: bool) -> Hsla {
    let w = ink.clamp(0.0, 1.0);
    hsla(0.0, 0.0, if dark { 1.0 - w } else { w }, alpha)
}

/// A primitive ready to paint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    /// A stroked line (its own path).
    Line {
        /// Start.
        from: (f32, f32),
        /// End.
        to: (f32, f32),
        /// Stroke width.
        width: f32,
        /// Color.
        color: Hsla,
    },
    /// A filled dot (a fully rounded quad).
    Dot {
        /// Center.
        center: (f32, f32),
        /// Radius.
        radius: f32,
        /// Color.
        color: Hsla,
    },
}

/// The paint plan of a frame (R13): all lines in frame order, then all dots in frame order;
/// primitives with alpha below 0.02 skipped; dot radius `max(r, r_min) * scale`; every
/// coordinate, radius and stroke width multiplied by `scale`.
pub fn plan_primitives(frame: &Frame, scale: f32, min_radius: f32, dark: bool) -> Vec<Primitive> {
    let scale = sanitize_scale(scale);
    let mut plan = Vec::with_capacity(frame.lines.len() + frame.dots.len());
    for line in frame.lines.iter().filter(|l| l.alpha >= ALPHA_CULL) {
        plan.push(Primitive::Line {
            from: (line.x1 * scale, line.y1 * scale),
            to: (line.x2 * scale, line.y2 * scale),
            width: line.width * scale,
            color: ink_color(line.ink, line.alpha, dark),
        });
    }
    for dot in frame.dots.iter().filter(|d| d.alpha >= ALPHA_CULL) {
        plan.push(Primitive::Dot {
            center: (dot.x * scale, dot.y * scale),
            radius: dot.radius.max(min_radius) * scale,
            color: ink_color(dot.ink, dot.alpha, dark),
        });
    }
    plan
}

fn paint_primitives(bounds: Bounds<Pixels>, plan: &[Primitive], window: &mut Window) {
    let origin = bounds.origin;
    // One paint layer for the whole frame: ordering cost is paid once.
    window.paint_layer(bounds, |window| {
        for primitive in plan {
            match *primitive {
                Primitive::Line {
                    from,
                    to,
                    width,
                    color,
                } => {
                    let mut builder = PathBuilder::stroke(px(width)).with_style(PathStyle::Stroke(
                        StrokeOptions::default()
                            .with_line_width(width)
                            .with_line_cap(LineCap::Round)
                            .with_line_join(LineJoin::Round),
                    ));
                    builder.move_to(point(origin.x + px(from.0), origin.y + px(from.1)));
                    builder.line_to(point(origin.x + px(to.0), origin.y + px(to.1)));
                    if let Ok(path) = builder.build() {
                        window.paint_path(path, color);
                    }
                }
                Primitive::Dot {
                    center,
                    radius,
                    color,
                } => {
                    let bounds = Bounds {
                        origin: point(
                            origin.x + px(center.0 - radius),
                            origin.y + px(center.1 - radius),
                        ),
                        size: size(px(radius * 2.0), px(radius * 2.0)),
                    };
                    window.paint_quad(fill(bounds, color).corner_radii(Corners::all(px(radius))));
                }
            }
        }
    });
}

fn frame_canvas(
    frame: Rc<RefCell<Frame>>,
    scale: f32,
    min_radius: f32,
    dark: bool,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let plan = plan_primitives(&frame.borrow(), scale, min_radius, dark);
            paint_primitives(bounds, &plan, window);
        },
    )
    .size_full()
}

/// The host-ticked variant (R15): paints one frame of `state` and `size` at animation time `t`
/// (seconds, unbounded) into the caller-owned reusable `buffer`. Reduced motion paints the
/// static frame at [`STATIC_TIME`]. The producer runs once, now.
#[allow(clippy::too_many_arguments)]
pub fn host_ticked(
    source: &mut dyn FrameSource,
    buffer: &Rc<RefCell<Frame>>,
    state: SharedString,
    size: SharedString,
    t: f32,
    reduced_motion: bool,
    scale: f32,
    min_radius: f32,
    dark: bool,
) -> impl IntoElement + use<> {
    let request = FrameRequest {
        state,
        size,
        time: if reduced_motion { STATIC_TIME } else { t },
        transition: Duration::ZERO,
        mic_level: 0.0,
        speaker_level: 0.0,
        reduced_motion,
    };
    source.generate(&request, &mut buffer.borrow_mut());
    frame_canvas(buffer.clone(), scale, min_radius, dark)
}

/// The retained animated-geometry view.
pub struct AnimatedGeometry {
    source: Box<dyn FrameSource>,
    state: SharedString,
    size: SharedString,
    scale: f32,
    theme: InkTheme,
    speed: f32,
    paused: bool,
    transition: Duration,
    reduced_motion: bool,
    fps: f32,
    pause_when_inactive: bool,
    visible: bool,
    mic: f32,
    speaker: f32,
    min_radius: f32,
    frame: Rc<RefCell<Frame>>,
    dirty: bool,
    /// Animation clock origin: `Some` while the clock runs.
    clock_start: Option<Instant>,
    /// Animation time held while not animating.
    held_time: f32,
    timer: Option<Task<()>>,
    producer_calls: usize,
    _activation: Option<Subscription>,
}

impl AnimatedGeometry {
    /// A view driven by `source`.
    pub fn new(source: impl FrameSource, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let activation = cx.observe_window_activation(window, |_, _, cx| cx.notify());
        Self {
            source: Box::new(source),
            state: SharedString::default(),
            size: SharedString::default(),
            scale: 1.0,
            theme: InkTheme::Auto,
            speed: 1.0,
            paused: false,
            transition: Duration::from_millis(250),
            reduced_motion: false,
            fps: DEFAULT_FPS,
            pause_when_inactive: true,
            visible: true,
            mic: 0.0,
            speaker: 0.0,
            min_radius: 1.0,
            frame: Rc::new(RefCell::new(Frame::new())),
            dirty: true,
            clock_start: None,
            held_time: 0.0,
            timer: None,
            producer_calls: 0,
            _activation: Some(activation),
        }
    }

    fn change<T: PartialEq>(slot: &mut T, value: T, dirty: &mut bool, cx: &mut Context<Self>) {
        if *slot != value {
            *slot = value;
            *dirty = true;
            cx.notify();
        }
    }

    /// Sets the producer state.
    pub fn set_state(&mut self, state: impl Into<SharedString>, cx: &mut Context<Self>) {
        Self::change(&mut self.state, state.into(), &mut self.dirty, cx);
    }

    /// Sets the size preset.
    pub fn set_size(&mut self, size: impl Into<SharedString>, cx: &mut Context<Self>) {
        Self::change(&mut self.size, size.into(), &mut self.dirty, cx);
    }

    /// Sets the uniform magnification (sanitized to `[0.25, 8]`).
    pub fn set_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        Self::change(&mut self.scale, sanitize_scale(scale), &mut self.dirty, cx);
    }

    /// Sets the ink direction.
    pub fn set_theme(&mut self, theme: InkTheme, cx: &mut Context<Self>) {
        Self::change(&mut self.theme, theme, &mut self.dirty, cx);
    }

    /// Sets the animation speed multiplier (non-finite or negative becomes 1).
    pub fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        let speed = if speed.is_finite() && speed >= 0.0 {
            speed
        } else {
            1.0
        };
        Self::change(&mut self.speed, speed, &mut self.dirty, cx);
    }

    /// Pauses or resumes animation.
    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        Self::change(&mut self.paused, paused, &mut self.dirty, cx);
    }

    /// Sets the duration of state crossfades.
    pub fn set_state_transition(&mut self, duration: Duration, cx: &mut Context<Self>) {
        Self::change(&mut self.transition, duration, &mut self.dirty, cx);
    }

    /// Requests reduced motion (the system preference applies as well).
    pub fn set_reduced_motion(&mut self, reduced: bool, cx: &mut Context<Self>) {
        Self::change(&mut self.reduced_motion, reduced, &mut self.dirty, cx);
    }

    /// Sets the target frame rate (sanitized to `[1, 30]`, non-finite becomes 30).
    pub fn set_target_fps(&mut self, fps: f32, cx: &mut Context<Self>) {
        let fps = sanitize_fps(fps);
        if self.fps != fps {
            self.fps = fps;
            cx.notify();
        }
    }

    /// Whether an inactive window pauses animation (default true).
    pub fn set_pause_when_inactive(&mut self, pause: bool, cx: &mut Context<Self>) {
        Self::change(&mut self.pause_when_inactive, pause, &mut self.dirty, cx);
    }

    /// Host-controlled visibility (default true).
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        Self::change(&mut self.visible, visible, &mut self.dirty, cx);
    }

    /// Sets the minimum dot radius (producer units).
    pub fn set_min_dot_radius(&mut self, radius: f32, cx: &mut Context<Self>) {
        Self::change(&mut self.min_radius, radius.max(0.0), &mut self.dirty, cx);
    }

    /// Sets the microphone and speaker levels, each clamped to `[0, 1]`. They influence only
    /// motion.
    pub fn set_audio_levels(&mut self, mic: f32, speaker: f32, cx: &mut Context<Self>) {
        let clean = |v: f32| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let (mic, speaker) = (clean(mic), clean(speaker));
        if self.mic != mic || self.speaker != speaker {
            self.mic = mic;
            self.speaker = speaker;
            self.dirty = true;
            cx.notify();
        }
    }

    /// The sanitized target frame rate.
    pub fn target_fps(&self) -> f32 {
        self.fps
    }

    /// Whether a tick timer is in flight. At most one ever is.
    pub fn has_timer(&self) -> bool {
        self.timer.is_some()
    }

    /// How many times the producer's math has run.
    pub fn producer_calls(&self) -> usize {
        self.producer_calls
    }

    /// Whether the animation clock currently runs.
    pub fn is_animating(&self) -> bool {
        self.clock_start.is_some()
    }

    /// The retained frame.
    pub fn frame(&self) -> Frame {
        self.frame.borrow().clone()
    }

    fn animating(&self, window: &Window, cx: &App) -> bool {
        self.visible
            && !self.paused
            && !(self.reduced_motion || cx.reduce_motion())
            && (!self.pause_when_inactive || window.is_window_active())
    }

    fn dark(&self, cx: &App) -> bool {
        match self.theme {
            InkTheme::Light => false,
            InkTheme::Dark => true,
            InkTheme::Auto => appearance::try_theme_of(cx)
                .map_or(true, |theme| theme.effective_appearance.is_dark()),
        }
    }

    fn arm_timer(&mut self, cx: &mut Context<Self>) {
        let interval = Duration::from_secs_f32(1.0 / self.fps);
        self.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(interval).await;
            this.update(cx, |this, cx| {
                this.timer = None;
                this.dirty = true;
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for AnimatedGeometry {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = cx.background_executor().now();
        let animating = self.animating(window, cx);
        let reduced = self.reduced_motion || cx.reduce_motion();
        if animating {
            if self.clock_start.is_none() {
                // Resume from the held time so the animation continues where it stopped.
                let held =
                    Duration::from_secs_f32((self.held_time / self.speed.max(1e-3)).max(0.0));
                self.clock_start = Some(now.checked_sub(held).unwrap_or(now));
                self.dirty = true;
            }
            if self.timer.is_none() {
                self.arm_timer(cx);
            }
        } else {
            self.timer = None;
            if self.clock_start.take().is_some() {
                self.source.clock_stopped();
                self.dirty = true;
            }
        }
        if self.dirty {
            let time = match self.clock_start {
                Some(start) => {
                    let time = now.saturating_duration_since(start).as_secs_f32() * self.speed;
                    self.held_time = time;
                    time
                }
                None if reduced => STATIC_TIME,
                None => self.held_time,
            };
            let request = FrameRequest {
                state: self.state.clone(),
                size: self.size.clone(),
                time,
                transition: self.transition,
                mic_level: self.mic,
                speaker_level: self.speaker,
                reduced_motion: reduced,
            };
            self.source.generate(&request, &mut self.frame.borrow_mut());
            self.producer_calls += 1;
            self.dirty = false;
        }
        let dark = self.dark(cx);
        div().size_full().child(frame_canvas(
            self.frame.clone(),
            self.scale,
            self.min_radius,
            dark,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a8_fps_sanitizes() {
        assert_eq!(sanitize_fps(0.0), 1.0);
        assert_eq!(sanitize_fps(240.0), 30.0);
        assert_eq!(sanitize_fps(f32::NAN), 30.0);
        assert_eq!(sanitize_fps(f32::INFINITY), 30.0);
        assert_eq!(sanitize_fps(12.0), 12.0);
        assert_eq!(sanitize_scale(100.0), 8.0);
        assert_eq!(sanitize_scale(0.0), 0.25);
        assert_eq!(sanitize_scale(f32::NAN), 1.0);
    }

    #[test]
    fn a9_painting_plan() {
        let mut frame = Frame::new();
        frame.lines.push(Line {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 0.0,
            width: 1.0,
            ink: 0.2,
            alpha: 0.5,
        });
        frame.lines.push(Line {
            x1: 0.0,
            y1: 0.0,
            x2: 0.0,
            y2: 10.0,
            width: 1.0,
            ink: 0.2,
            alpha: 0.019,
        });
        frame.lines.push(Line {
            x1: 1.0,
            y1: 1.0,
            x2: 5.0,
            y2: 5.0,
            width: 2.0,
            ink: 1.0,
            alpha: 1.0,
        });
        frame.dots.push(Dot {
            x: 2.0,
            y: 3.0,
            radius: 0.25,
            ink: 0.0,
            alpha: 1.0,
        });
        frame.dots.push(Dot {
            x: 2.0,
            y: 3.0,
            radius: 4.0,
            ink: 0.0,
            alpha: 0.019,
        });
        let plan = plan_primitives(&frame, 2.0, 1.0, false);
        // Skipped: alpha 0.019 line and dot. Lines first, as separate primitives.
        assert_eq!(plan.len(), 3);
        assert!(matches!(plan[0], Primitive::Line { width, .. } if width == 2.0));
        assert!(matches!(plan[1], Primitive::Line { to: (10.0, 10.0), width, .. } if width == 4.0));
        // A dot with r below r_min uses r_min, times the scale.
        assert!(
            matches!(plan[2], Primitive::Dot { radius, center: (4.0, 6.0), .. } if radius == 2.0)
        );
        let Primitive::Line { color, .. } = plan[0] else {
            unreachable!()
        };
        assert!((color.l - 0.2).abs() < 1e-6 && (color.a - 0.5).abs() < 1e-6);
        let dark = plan_primitives(&frame, 1.0, 1.0, true);
        let Primitive::Line { color, .. } = dark[0] else {
            unreachable!()
        };
        assert!((color.l - 0.8).abs() < 1e-6);
        // The scale is sanitized.
        let huge = plan_primitives(&frame, 1e9, 1.0, false);
        assert!(matches!(huge[0], Primitive::Line { width, .. } if width == 8.0));
    }
}
