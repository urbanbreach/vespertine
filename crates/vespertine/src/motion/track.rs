//! Declarative motion tracks: validation, sampling, retargeting and completion reports
//! (ANM-002 R1-R6).
//!
//! A [`MotionTrack`] is a pure state machine over an abstract monotonic time (a [`Duration`]
//! since an arbitrary origin), so it can be sampled at a frozen time in tests and fidelity
//! scenes. The [`MotionElement`](super::MotionElement) feeds it from the shared clock.

use std::time::Duration;

use super::easing::{Easing, MotionError};

/// Default transition duration of declarative tracks, in seconds.
pub const DEFAULT_DURATION_SECS: f64 = 0.3;

/// The animatable style properties of a motion track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MotionProperty {
    /// Pixel width.
    Width,
    /// Pixel height.
    Height,
    /// Top inset in pixels.
    Top,
    /// Right inset in pixels.
    Right,
    /// Bottom inset in pixels.
    Bottom,
    /// Left inset in pixels.
    Left,
    /// Border radius in pixels.
    BorderRadius,
    /// Scalar opacity.
    Opacity,
}

impl MotionProperty {
    /// All properties, in a fixed order.
    pub const ALL: [MotionProperty; 8] = [
        MotionProperty::Width,
        MotionProperty::Height,
        MotionProperty::Top,
        MotionProperty::Right,
        MotionProperty::Bottom,
        MotionProperty::Left,
        MotionProperty::BorderRadius,
        MotionProperty::Opacity,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            MotionProperty::Width => "width",
            MotionProperty::Height => "height",
            MotionProperty::Top => "top",
            MotionProperty::Right => "right",
            MotionProperty::Bottom => "bottom",
            MotionProperty::Left => "left",
            MotionProperty::BorderRadius => "borderRadius",
            MotionProperty::Opacity => "opacity",
        }
    }
}

fn representable_f32(value: f64) -> bool {
    value.is_finite() && (value as f32).is_finite()
}

fn option_bits(value: Option<f64>) -> Option<u64> {
    value.map(f64::to_bits)
}

/// A set of optional motion property values (a target or an initial style).
#[derive(Clone, Copy, Debug, Default)]
pub struct MotionProps {
    /// Width in pixels.
    pub width: Option<f64>,
    /// Height in pixels.
    pub height: Option<f64>,
    /// Border radius in pixels.
    pub border_radius: Option<f64>,
    /// Opacity from 0 to 1.
    pub opacity: Option<f64>,
    /// Left inset in pixels.
    pub left: Option<f64>,
    /// Right inset in pixels.
    pub right: Option<f64>,
    /// Top inset in pixels.
    pub top: Option<f64>,
    /// Bottom inset in pixels.
    pub bottom: Option<f64>,
}

impl PartialEq for MotionProps {
    /// Bitwise comparison, so that two descriptions carrying the same invalid (NaN) value count
    /// as unchanged.
    fn eq(&self, other: &Self) -> bool {
        MotionProperty::ALL
            .iter()
            .all(|&p| option_bits(self.get(p)) == option_bits(other.get(p)))
    }
}

impl MotionProps {
    /// An empty property set.
    pub const fn new() -> Self {
        Self {
            width: None,
            height: None,
            top: None,
            right: None,
            bottom: None,
            left: None,
            border_radius: None,
            opacity: None,
        }
    }

    /// Sets the width.
    pub const fn width(mut self, value: f64) -> Self {
        self.width = Some(value);
        self
    }

    /// Sets the height.
    pub const fn height(mut self, value: f64) -> Self {
        self.height = Some(value);
        self
    }

    /// Sets the top inset.
    pub const fn top(mut self, value: f64) -> Self {
        self.top = Some(value);
        self
    }

    /// Sets the right inset.
    pub const fn right(mut self, value: f64) -> Self {
        self.right = Some(value);
        self
    }

    /// Sets the bottom inset.
    pub const fn bottom(mut self, value: f64) -> Self {
        self.bottom = Some(value);
        self
    }

    /// Sets the left inset.
    pub const fn left(mut self, value: f64) -> Self {
        self.left = Some(value);
        self
    }

    /// Sets the border radius.
    pub const fn border_radius(mut self, value: f64) -> Self {
        self.border_radius = Some(value);
        self
    }

    /// Sets the opacity.
    pub const fn opacity(mut self, value: f64) -> Self {
        self.opacity = Some(value);
        self
    }

    /// Returns the value of one property.
    pub const fn get(&self, property: MotionProperty) -> Option<f64> {
        match property {
            MotionProperty::Width => self.width,
            MotionProperty::Height => self.height,
            MotionProperty::Top => self.top,
            MotionProperty::Right => self.right,
            MotionProperty::Bottom => self.bottom,
            MotionProperty::Left => self.left,
            MotionProperty::BorderRadius => self.border_radius,
            MotionProperty::Opacity => self.opacity,
        }
    }

    fn values(&self) -> [Option<f64>; 8] {
        MotionProperty::ALL.map(|p| self.get(p))
    }

    /// Validates every present value (ANM-002 R5): sizes and radius finite, nonnegative and
    /// representable as `f32`; insets finite and representable; opacity within 0 to 1.
    pub fn validate(&self) -> Result<(), MotionError> {
        for property in MotionProperty::ALL {
            let Some(value) = self.get(property) else {
                continue;
            };
            match property {
                MotionProperty::Width | MotionProperty::Height | MotionProperty::BorderRadius => {
                    if !representable_f32(value) || value < 0.0 {
                        return Err(MotionError::InvalidSize(property.name()));
                    }
                }
                MotionProperty::Top
                | MotionProperty::Right
                | MotionProperty::Bottom
                | MotionProperty::Left => {
                    if !representable_f32(value) {
                        return Err(MotionError::InvalidInset(property.name()));
                    }
                }
                MotionProperty::Opacity => {
                    if !(0.0..=1.0).contains(&value) {
                        return Err(MotionError::InvalidOpacity);
                    }
                }
            }
        }
        Ok(())
    }
}

/// The `initial` of a motion description.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MotionInitial {
    /// `initial: false`: the first target is displayed immediately.
    False,
    /// `initial: true`: always invalid (ANM-002 R1).
    True,
    /// Starting values for the first animation.
    Style(MotionProps),
}

/// Timing of a motion description: seconds, plus easing.
#[derive(Clone, Debug)]
pub struct MotionTransition {
    /// Duration in seconds (default 0.3).
    pub duration: f64,
    /// Delay before the animation starts, in seconds (default 0).
    pub delay: f64,
    /// Easing curve (default `easeOut`).
    pub easing: Easing,
}

impl Default for MotionTransition {
    fn default() -> Self {
        Self {
            duration: DEFAULT_DURATION_SECS,
            delay: 0.0,
            easing: Easing::EASE_OUT,
        }
    }
}

impl MotionTransition {
    /// A transition with the default duration, no delay and `easeOut` easing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the duration in seconds.
    pub fn duration(mut self, seconds: f64) -> Self {
        self.duration = seconds;
        self
    }

    /// Sets the delay in seconds.
    pub fn delay(mut self, seconds: f64) -> Self {
        self.delay = seconds;
        self
    }

    /// Sets the easing.
    pub fn easing(mut self, easing: impl Into<Easing>) -> Self {
        self.easing = easing.into();
        self
    }

    fn validate(&self) -> Result<(), MotionError> {
        let duration_ok = |seconds: f64| {
            seconds.is_finite() && seconds >= 0.0 && Duration::try_from_secs_f64(seconds).is_ok()
        };
        if !duration_ok(self.duration) {
            return Err(MotionError::InvalidDuration("duration"));
        }
        if !duration_ok(self.delay) {
            return Err(MotionError::InvalidDuration("delay"));
        }
        self.easing.validate()
    }

    fn same_as(&self, other: &Self) -> bool {
        self.duration.to_bits() == other.duration.to_bits()
            && self.delay.to_bits() == other.delay.to_bits()
            && easing_same(&self.easing, &other.easing)
    }
}

fn easing_same(a: &Easing, b: &Easing) -> bool {
    match (a, b) {
        (Easing::CubicBezier(a), Easing::CubicBezier(b)) => {
            a.x1.to_bits() == b.x1.to_bits()
                && a.y1.to_bits() == b.y1.to_bits()
                && a.x2.to_bits() == b.x2.to_bits()
                && a.y2.to_bits() == b.y2.to_bits()
        }
        (a, b) => a == b,
    }
}

/// A declarative motion description: `initial`, `animate`, `exit` and `transition`, plus a
/// completion generation and the presence state that selects between `animate` and `exit`.
#[derive(Clone, Debug)]
pub struct MotionDescription {
    /// Starting values for the first animation; see [`MotionInitial`].
    pub initial: Option<MotionInitial>,
    /// The target while the element is present.
    pub animate: MotionProps,
    /// The target while the element is exiting. Without one the element has no exit animation.
    pub exit: Option<MotionProps>,
    /// Whether the element is present. When `false` the exit target replaces `animate` and the
    /// track is an exit track.
    pub present: bool,
    /// Timing.
    pub transition: MotionTransition,
    /// A caller-supplied completion generation (see [`MotionCompletion`]).
    pub generation: u64,
}

impl MotionDescription {
    /// A present description animating toward `animate` with the default transition.
    pub fn new(animate: MotionProps) -> Self {
        Self {
            initial: None,
            animate,
            exit: None,
            present: true,
            transition: MotionTransition::default(),
            generation: 0,
        }
    }

    /// Sets the initial style.
    pub fn initial(mut self, initial: MotionInitial) -> Self {
        self.initial = Some(initial);
        self
    }

    /// Sets the exit target.
    pub fn exit(mut self, exit: MotionProps) -> Self {
        self.exit = Some(exit);
        self
    }

    /// Marks the element present or exiting.
    pub fn present(mut self, present: bool) -> Self {
        self.present = present;
        self
    }

    /// Sets the transition.
    pub fn transition(mut self, transition: MotionTransition) -> Self {
        self.transition = transition;
        self
    }

    /// Sets the completion generation.
    pub fn generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    /// Whether this is an exit track.
    pub fn is_exit(&self) -> bool {
        !self.present
    }

    /// The effective target: `animate` while present, otherwise the exit target (or `animate`
    /// again when there is none, which makes the exit a no-op that still completes).
    pub fn target(&self) -> &MotionProps {
        if self.present {
            &self.animate
        } else {
            self.exit.as_ref().unwrap_or(&self.animate)
        }
    }

    /// Validates the whole description (ANM-002 R5).
    pub fn validate(&self) -> Result<(), MotionError> {
        match &self.initial {
            Some(MotionInitial::True) => return Err(MotionError::InvalidInitial),
            Some(MotionInitial::Style(props)) => props.validate()?,
            Some(MotionInitial::False) | None => {}
        }
        self.animate.validate()?;
        if let Some(exit) = &self.exit {
            exit.validate()?;
        }
        self.transition.validate()
    }

    /// Whether the two descriptions are equal in every field including generation.
    pub fn same_as(&self, other: &Self) -> bool {
        self.initial == other.initial
            && self.animate == other.animate
            && self.exit == other.exit
            && self.present == other.present
            && self.generation == other.generation
            && self.transition.same_as(&other.transition)
    }
}

/// Raw progress per ANM-002 R3 for elapsed time `elapsed`, delay and duration. The value is
/// unclamped above (it exceeds 1 once finished).
pub fn raw_progress(elapsed: Duration, delay: Duration, duration: Duration) -> f64 {
    if duration.is_zero() {
        return if elapsed >= delay { 1.0 } else { 0.0 };
    }
    if elapsed <= delay {
        return 0.0;
    }
    (elapsed - delay).as_secs_f64() / duration.as_secs_f64()
}

/// The visible values a track contributes to its host element's style. A `None` property is
/// not overridden.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionStyle {
    /// Pixel width.
    pub width: Option<f32>,
    /// Pixel height.
    pub height: Option<f32>,
    /// Top inset in pixels.
    pub top: Option<f32>,
    /// Right inset in pixels.
    pub right: Option<f32>,
    /// Bottom inset in pixels.
    pub bottom: Option<f32>,
    /// Left inset in pixels.
    pub left: Option<f32>,
    /// Border radius in pixels.
    pub border_radius: Option<f32>,
    /// Opacity multiplier from 0 to 1.
    pub opacity: Option<f32>,
}

impl MotionStyle {
    fn from_values(values: [Option<f64>; 8]) -> Self {
        let f = |p: MotionProperty| values[p.index()].map(|v| v as f32);
        Self {
            width: f(MotionProperty::Width),
            height: f(MotionProperty::Height),
            top: f(MotionProperty::Top),
            right: f(MotionProperty::Right),
            bottom: f(MotionProperty::Bottom),
            left: f(MotionProperty::Left),
            border_radius: f(MotionProperty::BorderRadius),
            opacity: f(MotionProperty::Opacity),
        }
    }

    /// Returns the value of one property.
    pub fn get(&self, property: MotionProperty) -> Option<f32> {
        match property {
            MotionProperty::Width => self.width,
            MotionProperty::Height => self.height,
            MotionProperty::Top => self.top,
            MotionProperty::Right => self.right,
            MotionProperty::Bottom => self.bottom,
            MotionProperty::Left => self.left,
            MotionProperty::BorderRadius => self.border_radius,
            MotionProperty::Opacity => self.opacity,
        }
    }

    /// Applies this style to a host element's style, after the element's own style: width and
    /// height as pixel sizes, insets, border radius, and opacity multiplying through the
    /// element's own opacity.
    pub fn apply(&self, style: &mut crate::StyleRefinement) {
        use crate::{AbsoluteLength, Length, px};
        let length = |value: f32| Some(Length::from(px(value)));
        if let Some(value) = self.width {
            style.size.width = length(value);
        }
        if let Some(value) = self.height {
            style.size.height = length(value);
        }
        if let Some(value) = self.top {
            style.inset.top = length(value);
        }
        if let Some(value) = self.right {
            style.inset.right = length(value);
        }
        if let Some(value) = self.bottom {
            style.inset.bottom = length(value);
        }
        if let Some(value) = self.left {
            style.inset.left = length(value);
        }
        if let Some(value) = self.border_radius {
            let radius = Some(AbsoluteLength::Pixels(px(value)));
            style.corner_radii.top_left = radius;
            style.corner_radii.top_right = radius;
            style.corner_radii.bottom_right = radius;
            style.corner_radii.bottom_left = radius;
        }
        if let Some(value) = self.opacity {
            style.opacity = Some(style.opacity.unwrap_or(1.0) * value);
        }
    }
}

/// A completion report: the track finished (or never needed to run) and owes its generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionCompletion {
    /// The generation of the description that completed. A presence manager rejects reports
    /// whose generation is not current.
    pub generation: u64,
    /// Whether the completed description was an exit track.
    pub exit: bool,
}

/// The result of sampling a track at one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionSample {
    /// The visible style.
    pub style: MotionStyle,
    /// Whether the track is active and needs another frame.
    pub active: bool,
    /// A completion owed by the track, reported exactly once.
    pub completion: Option<MotionCompletion>,
}

#[derive(Clone, Debug)]
struct Running {
    start: [Option<f64>; 8],
    target: [Option<f64>; 8],
    started: Duration,
    delay: Duration,
    duration: Duration,
    easing: Easing,
}

impl Running {
    fn raw(&self, now: Duration) -> f64 {
        raw_progress(now.saturating_sub(self.started), self.delay, self.duration)
    }

    fn differs(&self) -> bool {
        (0..8).any(|i| matches!((self.start[i], self.target[i]), (Some(s), Some(t)) if s != t))
    }

    /// Visible values: interpolated for properties with a target (reaching the target exactly
    /// at the end), `None` otherwise.
    fn visible(&self, now: Duration, settled: bool) -> [Option<f64>; 8] {
        let raw = if settled { 1.0 } else { self.raw(now) };
        let eased = f64::from(self.easing.apply(raw.clamp(0.0, 1.0) as f32));
        let mut out = [None; 8];
        for i in 0..8 {
            let Some(target) = self.target[i] else {
                continue;
            };
            let start = self.start[i].unwrap_or(target);
            out[i] = Some(if raw >= 1.0 {
                target
            } else {
                start + (target - start) * eased
            });
        }
        out
    }

    /// Values a replacing description starts from: the visible value where there is a target,
    /// otherwise the retained start.
    fn carry(&self, now: Duration) -> [Option<f64>; 8] {
        let visible = self.visible(now, false);
        std::array::from_fn(|i| visible[i].or(self.start[i]))
    }
}

/// A declarative motion track.
#[derive(Clone, Debug, Default)]
pub struct MotionTrack {
    description: Option<MotionDescription>,
    running: Option<Running>,
    owed: bool,
    reported: bool,
}

impl MotionTrack {
    /// Creates an empty track; call [`MotionTrack::update`] with the first description.
    pub fn new() -> Self {
        Self::default()
    }

    /// The last description given to the track.
    pub fn description(&self) -> Option<&MotionDescription> {
        self.description.as_ref()
    }

    /// Whether the last description was valid (an invalid one contributes no style and no time).
    pub fn is_valid(&self) -> bool {
        self.running.is_some()
    }

    /// Supplies the current description at time `now`.
    ///
    /// An unchanged description (equal in every field including generation) does nothing.
    /// Otherwise the track restarts at `now` from its currently visible values (ANM-002 R2); the
    /// first description starts from its initial style, or at its target.
    pub fn update(&mut self, description: &MotionDescription, now: Duration) {
        if self
            .description
            .as_ref()
            .is_some_and(|previous| previous.same_as(description))
        {
            return;
        }
        let generation_changed = self
            .description
            .as_ref()
            .is_some_and(|previous| previous.generation != description.generation);
        let carried = self.running.as_ref().map(|running| running.carry(now));
        let was_valid = carried.is_some();
        let is_first = self.description.is_none();

        self.running = match description.validate() {
            Err(_) => None,
            Ok(()) => {
                let target = description.target().values();
                let mut start = match (&carried, &description.initial) {
                    (Some(carried), _) => *carried,
                    (None, Some(MotionInitial::Style(initial))) if is_first || !was_valid => {
                        initial.values()
                    }
                    _ => [None; 8],
                };
                for i in 0..8 {
                    if start[i].is_none() {
                        start[i] = target[i];
                    }
                }
                let transition = &description.transition;
                Some(Running {
                    start,
                    target,
                    started: now,
                    delay: Duration::from_secs_f64(transition.delay),
                    duration: Duration::from_secs_f64(transition.duration),
                    easing: transition.easing.clone(),
                })
            }
        };
        self.owed |= description.is_exit() || generation_changed;
        self.reported = false;
        self.description = Some(description.clone());
    }

    /// Samples the track at time `now`. Under reduced motion the track is settled at its target.
    ///
    /// Completion is reported at most once per accepted description (and again only after a new
    /// description is accepted), and never while the track is still running.
    pub fn sample(&mut self, now: Duration, reduced_motion: bool) -> MotionSample {
        let (style, active) = match &self.running {
            None => (MotionStyle::default(), false),
            Some(running) => {
                if running.differs() && !self.reported {
                    self.owed = true;
                }
                let active = !reduced_motion && running.differs() && running.raw(now) < 1.0;
                (
                    MotionStyle::from_values(running.visible(now, reduced_motion)),
                    active,
                )
            }
        };
        let completion = if !active && self.owed {
            self.owed = false;
            self.reported = true;
            self.description
                .as_ref()
                .map(|description| MotionCompletion {
                    generation: description.generation,
                    exit: description.is_exit(),
                })
        } else {
            None
        };
        MotionSample {
            style,
            active,
            completion,
        }
    }
}
