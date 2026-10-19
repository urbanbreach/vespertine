//! The slider (CMP-001 R15-R21): a continuous or discrete value on a numeric range, set by
//! pointer drag or keyboard.
//!
//! # Pure helpers
//!
//! [`SliderRange`] holds the numeric model and implements the value rules without any window:
//!
//! * [`SliderRange::normalize`] (R15): a non-finite input becomes the declared default;
//!   otherwise the input is clamped to the range; with a step the value becomes
//!   `minimum + round((value - minimum) / step) * step` and is clamped again; with a stop list
//!   the value is the stop nearest to the input.
//! * [`fraction_for_x`], [`SliderRange::value_for_x`], [`stop_index_for_x`] (R16): the interactive
//!   surface spans the whole control box; the thumb centre travels between `inset` and
//!   `width - inset` (inset = half the thumb width, 7 px for the default 14 px thumb), so the
//!   fraction of a pointer x is `clamp((x - left - inset) / max(width - 2 * inset, 1), 0, 1)`.
//!   For `n >= 1` stops, stop `i` sits at fraction `i / (n - 1)` (0 when `n = 1`) and the selected
//!   index is `round(fraction * (n - 1))`; with no stops there is no index and no change.
//! * [`SliderRange::step_value`] (R18): the keyboard step is the declared step, one stop for a
//!   stop list, or one hundredth of the range for a continuous slider without a step.
//!
//! # The control
//!
//! [`Slider`] draws the rail, the filled portion, optional stops, the thumb and optional
//! furniture (label, current-value readout, minimum and maximum labels, reset action):
//!
//! * A left press focuses the slider, sets the value from the press position at once and begins a
//!   drag that keeps following the pointer anywhere in the window (pointer capture, INP-001)
//!   until release. Press and drag consume their events and prevent the default; other buttons do
//!   nothing. Rapid pointer moves are coalesced to one owner notification per frame while the
//!   painted thumb follows the newest pending value. An input that normalizes to the current
//!   value neither notifies the owner nor repaints.
//! * Keyboard (focused): Left and Down step down, Right and Up step up, Home and End jump to the
//!   ends; handled keys prevent the default and stop propagation, all other keys pass through;
//!   values clamp at the ends.
//! * Accessibility: role slider with label, numeric value, minimum, maximum, step and the
//!   owner-formatted value text; a disabled slider reports disabled, takes no focus and ignores
//!   pointer and key input.
//! * Furniture revealed on interaction is shown while the slider is hovered, pressed or dragged,
//!   or focused after keyboard use, and keeps its layout space while hidden.
//! * Stops draw at their fractions (both ends included); the filled portion runs from the rail
//!   start to a rail radius past the thumb centre, and stops at or below the *animated* thumb
//!   fraction use the fill color. A discrete thumb glides between stops with the retargetable
//!   scalar of [`crate::controls::motion`] (reduced motion snaps).

use std::{cell::Cell, rc::Rc, time::Duration};

use vespertine::{
    AnyElement, App, Bounds, Context, ElementId, Entity, FocusHandle, InteractiveElement,
    IntoElement, MouseButton, Orientation, ParentElement, Pixels, RenderOnce, Role, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};

use super::{
    Button, ScalarMotion, effective_disabled, motion::SWITCH_THUMB_DURATION, track_bounds,
};
use crate::tokens::ComponentTokens;

/// Default width of the slider's control surface.
pub const SLIDER_WIDTH: Pixels = px(240.);
/// Default height of the slider's control surface.
pub const SLIDER_HEIGHT: Pixels = px(28.);
/// Default thumb width (and height) of the continuous slider.
pub const THUMB_WIDTH: Pixels = px(14.);
/// Thumb width of the wide-thumb preset.
pub const WIDE_THUMB_WIDTH: Pixels = px(44.);
/// Height of the rail.
pub const RAIL_HEIGHT: Pixels = px(4.);
/// Distance of the rail's top edge from the top of the surface.
pub const RAIL_TOP: Pixels = px(12.);
/// How far the thumb's top edge sits above the rail's top edge.
pub const THUMB_RISE: Pixels = px(5.);
/// Diameter of a stop mark.
pub const STOP_SIZE: Pixels = px(8.);
/// Height of the furniture rows above and below the surface.
pub const FURNITURE_HEIGHT: Pixels = px(20.);

/// The numeric model of a slider and its value rules.
#[derive(Clone, Debug, PartialEq)]
pub struct SliderRange {
    /// Lower end of the range.
    pub minimum: f64,
    /// Upper end of the range.
    pub maximum: f64,
    /// Value granularity, when the slider is stepped.
    pub step: Option<f64>,
    /// Explicit stops (an ascending list drawn on the rail), when the slider is discrete.
    pub stops: Option<Vec<f64>>,
    /// Value used for non-finite input and restored by the reset action.
    pub default: f64,
}

impl SliderRange {
    /// A continuous range whose default is its minimum.
    pub fn new(minimum: f64, maximum: f64) -> Self {
        Self {
            minimum,
            maximum,
            step: None,
            stops: None,
            default: minimum,
        }
    }

    /// Sets the step.
    pub fn with_step(mut self, step: f64) -> Self {
        self.step = Some(step);
        self
    }

    /// Sets the stop list.
    pub fn with_stops(mut self, stops: impl Into<Vec<f64>>) -> Self {
        self.stops = Some(stops.into());
        self
    }

    /// Sets the declared default value.
    pub fn with_default(mut self, default: f64) -> Self {
        self.default = default;
        self
    }

    /// The nonempty stop list, if the slider is discrete.
    fn stop_list(&self) -> Option<&[f64]> {
        self.stops.as_deref().filter(|stops| !stops.is_empty())
    }

    /// Whether the slider has an explicit stop list (possibly empty).
    pub fn has_stops(&self) -> bool {
        self.stops.is_some()
    }

    /// The number of stops (0 without a stop list).
    pub fn stop_count(&self) -> usize {
        self.stops.as_ref().map_or(0, Vec::len)
    }

    fn clamp(&self, value: f64) -> f64 {
        if self.maximum <= self.minimum {
            self.minimum
        } else {
            value.clamp(self.minimum, self.maximum)
        }
    }

    /// Normalizes an input value (R15). See the [module documentation](self).
    pub fn normalize(&self, input: f64) -> f64 {
        if !input.is_finite() {
            return self.default;
        }
        if let Some(stops) = self.stop_list() {
            return stops[nearest_index(stops, input)];
        }
        let mut value = self.clamp(input);
        if let Some(step) = self.step.filter(|step| step.is_finite() && *step > 0.0) {
            value = self.clamp(self.minimum + ((value - self.minimum) / step).round() * step);
        }
        value
    }

    /// The fraction between 0 and 1 at which the thumb of `value` sits: for a stop list the
    /// fraction of the nearest stop, otherwise the position of `value` within the range.
    pub fn fraction_of(&self, value: f64) -> f32 {
        if let Some(stops) = self.stop_list() {
            return stop_fraction(nearest_index(stops, value), stops.len());
        }
        let span = self.maximum - self.minimum;
        if span <= 0.0 || !value.is_finite() {
            return 0.0;
        }
        (((value - self.minimum) / span).clamp(0.0, 1.0)) as f32
    }

    /// The value a pointer at `x` selects on a surface starting at `left` with the given
    /// `width` and thumb `inset` (R16), normalized. `None` when the stop list is empty
    /// (there is nothing to select).
    pub fn value_for_x(&self, x: f32, left: f32, width: f32, inset: f32) -> Option<f64> {
        if self.stops.as_ref().is_some_and(Vec::is_empty) {
            return None;
        }
        if let Some(stops) = self.stop_list() {
            let index = stop_index_for_x(x, left, width, inset, stops.len())?;
            return Some(stops[index]);
        }
        let fraction = f64::from(fraction_for_x(x, left, width, inset));
        Some(self.normalize(self.minimum + fraction * (self.maximum - self.minimum)))
    }

    /// The keyboard step size of a continuous slider: the declared step, or one hundredth of
    /// the range.
    pub fn keyboard_step(&self) -> f64 {
        self.step
            .filter(|step| step.is_finite() && *step > 0.0)
            .unwrap_or((self.maximum - self.minimum) / 100.0)
    }

    /// The value one keyboard step from `current` in `direction` (negative: down), normalized.
    /// For a stop list the step is one stop; the result clamps at the ends, never wrapping.
    pub fn step_value(&self, current: f64, direction: i32) -> f64 {
        if let Some(stops) = self.stop_list() {
            let index = nearest_index(stops, current);
            let target = if direction < 0 {
                index.saturating_sub(1)
            } else {
                (index + 1).min(stops.len() - 1)
            };
            return stops[target];
        }
        let current = if current.is_finite() {
            current
        } else {
            self.default
        };
        self.normalize(current + f64::from(direction.signum()) * self.keyboard_step())
    }

    /// The value of the Home key.
    pub fn home_value(&self) -> f64 {
        self.normalize(self.minimum)
    }

    /// The value of the End key.
    pub fn end_value(&self) -> f64 {
        self.normalize(self.maximum)
    }
}

/// Index of the entry of `stops` nearest to `input` (the first one wins ties).
fn nearest_index(stops: &[f64], input: f64) -> usize {
    let mut best = 0;
    let mut best_distance = f64::INFINITY;
    for (index, stop) in stops.iter().enumerate() {
        let distance = (stop - input).abs();
        if distance < best_distance {
            best = index;
            best_distance = distance;
        }
    }
    best
}

/// The fraction of the thumb travel a pointer at `x` selects (R16):
/// `clamp((x - left - inset) / max(width - 2 * inset, 1), 0, 1)`.
pub fn fraction_for_x(x: f32, left: f32, width: f32, inset: f32) -> f32 {
    ((x - left - inset) / (width - 2.0 * inset).max(1.0)).clamp(0.0, 1.0)
}

/// The fraction at which stop `index` of `count` stops sits: `index / (count - 1)`, 0 for a
/// single stop.
pub fn stop_fraction(index: usize, count: usize) -> f32 {
    if count <= 1 {
        0.0
    } else {
        index as f32 / (count - 1) as f32
    }
}

/// The stop a pointer at `x` selects among `count` stops: `round(fraction * (count - 1))`, a
/// halfway position rounding to the upper stop. `None` for zero stops.
pub fn stop_index_for_x(x: f32, left: f32, width: f32, inset: f32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let fraction = fraction_for_x(x, left, width, inset);
    Some((fraction * (count - 1) as f32).round() as usize)
}

/// Layout of the rail and thumb inside the control surface (R16, R21): the thumb centre travels
/// between `inset` and `width - inset`, the filled portion runs from the rail start to a rail
/// radius past the thumb centre, and stop `i` of `n` sits on the thumb's travel at `i / (n - 1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SliderGeometry {
    /// Width of the control surface.
    pub width: f32,
    /// Distance of the thumb centre from either end at its extremes (half the thumb width).
    pub inset: f32,
    /// Height of the rail.
    pub rail_height: f32,
}

impl SliderGeometry {
    /// The horizontal travel of the thumb centre.
    pub fn travel(&self) -> f32 {
        (self.width - 2.0 * self.inset).max(1.0)
    }

    /// The x of the thumb centre (from the surface's left edge) at `fraction`.
    pub fn thumb_center(&self, fraction: f32) -> f32 {
        self.inset + self.travel() * fraction
    }

    /// The x at which the filled portion ends: a rail radius past the thumb centre.
    pub fn fill_end(&self, fraction: f32) -> f32 {
        (self.thumb_center(fraction) + self.rail_height * 0.5).min(self.width)
    }

    /// The x of the centre of stop `index` of `count`.
    pub fn stop_center(&self, index: usize, count: usize) -> f32 {
        self.thumb_center(stop_fraction(index, count))
    }

    /// Whether stop `index` of `count` is drawn filled: its fraction is at or below the
    /// (animated) thumb `fraction`.
    pub fn stop_filled(&self, index: usize, count: usize, fraction: f32) -> bool {
        stop_fraction(index, count) <= fraction + 1e-4
    }
}

/// State of a slider that lives across frames.
struct SliderState {
    focus: FocusHandle,
    /// Tracks "focus is inside the slider" (the slider surface or its reset action).
    within: FocusHandle,
    /// Bounds of the interactive surface, recorded each frame.
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The value the owner holds, as of the last render.
    shown: f64,
    /// A press or drag is in progress.
    dragging: bool,
    /// The newest value not yet delivered to the owner.
    pending: Option<f64>,
    /// A delivery of `pending` is scheduled for the next frame.
    flush_scheduled: bool,
    hovered: bool,
    keyboard_active: bool,
    motion: ScalarMotion,
}

type ChangeHandler = Rc<dyn Fn(f64, &mut Window, &mut App)>;
type FormatHandler = Rc<dyn Fn(f64) -> String>;

impl SliderState {
    fn new(cx: &mut Context<Self>, fraction: f32, duration: Duration) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        let within = cx.focus_handle().tab_stop(false);
        Self {
            focus,
            within,
            bounds: Rc::default(),
            shown: 0.0,
            dragging: false,
            pending: None,
            flush_scheduled: false,
            hovered: false,
            keyboard_active: false,
            motion: ScalarMotion::new(fraction, duration),
        }
    }

    /// The value the thumb displays: the newest pending value, else the owner's.
    fn displayed(&self) -> f64 {
        self.pending.unwrap_or(self.shown)
    }
}

/// Delivers `value` to the owner at once, superseding any pending value. An input equal to the
/// owner's value notifies nobody.
fn deliver(
    state: &Entity<SliderState>,
    value: f64,
    on_change: &Option<ChangeHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    let changed = state.update(cx, |state, _| {
        state.pending = None;
        value != state.shown
    });
    if changed && let Some(on_change) = on_change {
        on_change(value, window, cx);
    }
}

/// Records `value` as the newest pending value of a drag (the painted thumb follows it) and
/// schedules its delivery for the next frame; further values before then replace it, so the
/// owner is notified at most once per frame.
fn schedule(
    state: &Entity<SliderState>,
    value: f64,
    on_change: &Option<ChangeHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    let spawn = state.update(cx, |state, cx| {
        if value == state.displayed() {
            return false;
        }
        state.pending = Some(value);
        cx.notify();
        !std::mem::replace(&mut state.flush_scheduled, true)
    });
    if spawn {
        let state = state.clone();
        let on_change = on_change.clone();
        window.on_next_frame(move |window, cx| flush(&state, &on_change, window, cx));
    }
}

/// Delivers the pending value, if the owner does not already hold it.
fn flush(
    state: &Entity<SliderState>,
    on_change: &Option<ChangeHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    let pending = state.update(cx, |state, _| {
        state.flush_scheduled = false;
        state.pending.take()
    });
    if let Some(value) = pending {
        deliver(state, value, on_change, window, cx);
    }
}

/// Ends the drag and delivers the final pending value without waiting for the next frame.
fn end_drag(
    state: &Entity<SliderState>,
    on_change: &Option<ChangeHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    let was_dragging = state.update(cx, |state, cx| {
        let was = std::mem::replace(&mut state.dragging, false);
        if was {
            cx.notify();
        }
        was
    });
    if was_dragging {
        flush(state, on_change, window, cx);
    }
}

/// The slider. See the [module documentation](self).
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    range: SliderRange,
    value: f64,
    on_change: Option<ChangeHandler>,
    on_reset: Option<Rc<dyn Fn(f64, &mut Window, &mut App)>>,
    label: Option<SharedString>,
    accessible_label: Option<SharedString>,
    value_text: Option<FormatHandler>,
    show_value: bool,
    range_labels: Option<(SharedString, SharedString)>,
    reset_label: SharedString,
    reveal_on_interaction: bool,
    disabled: bool,
    width: Pixels,
    thumb_width: Pixels,
    motion_duration: Duration,
    tokens: Option<ComponentTokens>,
}

impl Slider {
    /// A slider with the given identity over `range`, at the range's default value.
    pub fn new(id: impl Into<ElementId>, range: SliderRange) -> Self {
        let value = range.default;
        Self {
            id: id.into(),
            range,
            value,
            on_change: None,
            on_reset: None,
            label: None,
            accessible_label: None,
            value_text: None,
            show_value: false,
            range_labels: None,
            reset_label: "Reset".into(),
            reveal_on_interaction: false,
            disabled: false,
            width: SLIDER_WIDTH,
            thumb_width: THUMB_WIDTH,
            motion_duration: SWITCH_THUMB_DURATION,
            tokens: None,
        }
    }

    /// The current value, owned by the caller.
    pub fn value(mut self, value: f64) -> Self {
        self.value = value;
        self
    }

    /// Called with each new normalized value from pointer or keyboard input: at most once per
    /// frame during rapid pointer moves, never for an input that normalizes to the current
    /// value.
    pub fn on_change(mut self, handler: impl Fn(f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Shows a reset action that calls `handler` with the default value to restore.
    pub fn on_reset(mut self, handler: impl Fn(f64, &mut Window, &mut App) + 'static) -> Self {
        self.on_reset = Some(Rc::new(handler));
        self
    }

    /// The visible label, which is also the accessible name unless
    /// [`Slider::accessible_label`] is given.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// An explicit accessible name.
    pub fn accessible_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessible_label = Some(label.into());
        self
    }

    /// Formats the human-readable value text (for example a percentage or a pixel count),
    /// used for the readout and for assistive technology.
    pub fn value_text(mut self, format: impl Fn(f64) -> String + 'static) -> Self {
        self.value_text = Some(Rc::new(format));
        self
    }

    /// Shows the current-value readout.
    pub fn show_value(mut self, show: bool) -> Self {
        self.show_value = show;
        self
    }

    /// Shows minimum and maximum range labels.
    pub fn range_labels(
        mut self,
        minimum: impl Into<SharedString>,
        maximum: impl Into<SharedString>,
    ) -> Self {
        self.range_labels = Some((minimum.into(), maximum.into()));
        self
    }

    /// The text of the reset action (default "Reset").
    pub fn reset_label(mut self, label: impl Into<SharedString>) -> Self {
        self.reset_label = label.into();
        self
    }

    /// Reveals the readout, range labels and reset action only while the slider is hovered,
    /// pressed or dragged, or focused after keyboard use. Hidden furniture keeps its space.
    pub fn reveal_on_interaction(mut self, reveal: bool) -> Self {
        self.reveal_on_interaction = reveal;
        self
    }

    /// Disables the slider: no focus, no pointer or key input, disabled state exposed.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// The width of the control surface (default 240 px).
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// The thumb width (default 14 px; the inset is half of it).
    pub fn thumb_width(mut self, width: Pixels) -> Self {
        self.thumb_width = width;
        self
    }

    /// The wide-thumb preset: a 44 px thumb with a 22 px inset.
    pub fn wide_thumb(self) -> Self {
        self.thumb_width(WIDE_THUMB_WIDTH)
    }

    /// The duration of the discrete thumb's glide between stops (default 180 ms).
    pub fn motion_duration(mut self, duration: Duration) -> Self {
        self.motion_duration = duration;
        self
    }

    /// Overrides the token roles (the default is [`ComponentTokens::current`]).
    pub fn tokens(mut self, tokens: ComponentTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    fn format(&self, value: f64) -> String {
        match &self.value_text {
            Some(format) => format(value),
            None => format!("{value}"),
        }
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = self
            .tokens
            .clone()
            .unwrap_or_else(|| ComponentTokens::current(cx));
        let disabled = effective_disabled(self.disabled, cx);
        let range = self.range.clone();
        let value = range.normalize(self.value);
        let inset = self.thumb_width * 0.5;
        let width = self.width;

        let initial_fraction = range.fraction_of(value);
        let duration = self.motion_duration;
        let state = window.use_keyed_state((self.id.clone(), "slider"), cx, move |_, cx| {
            SliderState::new(cx, initial_fraction, duration)
        });

        // The owner's value as of this frame; a pending value the owner has caught up with is
        // done.
        let (focus, within, bounds, displayed, dragging, hovered, keyboard_active) =
            state.update(cx, |state, _| {
                state.shown = value;
                if state.pending == Some(value) {
                    state.pending = None;
                }
                state.motion.set_duration(duration);
                (
                    state.focus.clone(),
                    state.within.clone(),
                    state.bounds.clone(),
                    state.displayed(),
                    state.dragging,
                    state.hovered,
                    state.keyboard_active,
                )
            });
        let focus = focus.tab_stop(!disabled);
        let revealed = !self.reveal_on_interaction
            || dragging
            || hovered
            || (keyboard_active && within.contains_focused(window, cx));

        // The thumb fraction, animated for a discrete slider.
        let target_fraction = range.fraction_of(displayed);
        let fraction = if range.has_stops() {
            state
                .update(cx, |state, cx| {
                    state.motion.advance(target_fraction, window, cx)
                })
                .clamp(0.0, 1.0)
        } else {
            target_fraction
        };

        let geometry = SliderGeometry {
            width: f32::from(width),
            inset: f32::from(inset),
            rail_height: f32::from(RAIL_HEIGHT),
        };
        let thumb_center = px(geometry.thumb_center(fraction));
        let fill_end = px(geometry.fill_end(fraction));
        let rail_radius = RAIL_HEIGHT * 0.5;
        let keyboard_visible = focus.is_focused(window) && window.last_input_was_keyboard();

        let name = self.accessible_label.clone().or_else(|| self.label.clone());
        let value_text = self.format(displayed);
        let step = if range.has_stops() {
            // A stop list steps one stop; report the smallest gap as the numeric step.
            range
                .stops
                .as_deref()
                .and_then(|stops| {
                    stops
                        .windows(2)
                        .map(|pair| (pair[1] - pair[0]).abs())
                        .fold(None, |min: Option<f64>, gap| {
                            Some(min.map_or(gap, |min| min.min(gap)))
                        })
                })
                .unwrap_or(1.0)
        } else {
            range.keyboard_step()
        };

        // Rail, fill, stops, thumb.
        let mut surface = div()
            .id((self.id.clone(), "surface"))
            .relative()
            .flex_none()
            .w(width)
            .h(SLIDER_HEIGHT)
            .role(Role::Slider)
            .aria_orientation(Orientation::Horizontal)
            .aria_numeric_value(displayed)
            .aria_min_numeric_value(range.minimum)
            .aria_max_numeric_value(range.maximum)
            .aria_numeric_value_step(step)
            .aria_value(value_text.clone())
            .aria_disabled(disabled)
            .when_some(name, |surface, name| surface.aria_label(name))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(RAIL_TOP)
                    .w(width)
                    .h(RAIL_HEIGHT)
                    .rounded(rail_radius)
                    .bg(tokens.border),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(RAIL_TOP)
                    .w(fill_end)
                    .h(RAIL_HEIGHT)
                    .rounded(rail_radius)
                    .bg(tokens.accent),
            );

        if let Some(stops) = range.stops.as_deref() {
            for index in 0..stops.len() {
                let filled = geometry.stop_filled(index, stops.len(), fraction);
                let center = px(geometry.stop_center(index, stops.len()));
                surface = surface.child(
                    div()
                        .absolute()
                        .left(center - STOP_SIZE * 0.5)
                        .top(RAIL_TOP + rail_radius - STOP_SIZE * 0.5)
                        .size(STOP_SIZE)
                        .rounded_full()
                        .bg(if filled { tokens.accent } else { tokens.border }),
                );
            }
        }

        let thumb_height = THUMB_WIDTH;
        let thumb_top = RAIL_TOP - THUMB_RISE;
        surface = surface
            .child(
                div()
                    .absolute()
                    .left(thumb_center - self.thumb_width * 0.5)
                    .top(thumb_top)
                    .w(self.thumb_width)
                    .h(thumb_height)
                    .rounded_full()
                    .border_1()
                    .border_color(tokens.accent)
                    .bg(tokens.accent_contrast),
            )
            .when(keyboard_visible, |surface| {
                surface.child(
                    div()
                        .absolute()
                        .left(thumb_center - self.thumb_width * 0.5 - px(3.))
                        .top(thumb_top - px(3.))
                        .w(self.thumb_width + px(6.))
                        .h(thumb_height + px(6.))
                        .rounded_full()
                        .border_2()
                        .border_color(tokens.focus_ring),
                )
            })
            .child(track_bounds(bounds.clone()));

        if !disabled {
            let on_change = self.on_change.clone();
            surface = surface
                .track_focus(&focus)
                .cursor_pointer()
                // Pointer capture (INP-001): after a left press the drag follows this element
                // wherever the pointer goes in the window, until the button is released.
                .capture_pointer()
                .on_mouse_down(MouseButton::Left, {
                    let state = state.clone();
                    let bounds = bounds.clone();
                    let range = range.clone();
                    let on_change = on_change.clone();
                    let focus = focus.clone();
                    move |event, window, cx| {
                        window.prevent_default();
                        focus.focus(window, cx);
                        state.update(cx, |state, cx| {
                            state.dragging = true;
                            state.keyboard_active = false;
                            cx.notify();
                        });
                        let area = bounds.get();
                        if let Some(value) = range.value_for_x(
                            f32::from(event.position.x),
                            f32::from(area.origin.x),
                            f32::from(area.size.width),
                            f32::from(inset),
                        ) {
                            deliver(&state, value, &on_change, window, cx);
                        }
                    }
                })
                .on_mouse_move({
                    let state = state.clone();
                    let range = range.clone();
                    let on_change = on_change.clone();
                    move |event, window, cx| {
                        if !state.read(cx).dragging {
                            return;
                        }
                        if event.pressed_button != Some(MouseButton::Left) {
                            // The release was missed: the drag is over.
                            end_drag(&state, &on_change, window, cx);
                            return;
                        }
                        cx.stop_propagation();
                        let area = bounds.get();
                        if let Some(value) = range.value_for_x(
                            f32::from(event.position.x),
                            f32::from(area.origin.x),
                            f32::from(area.size.width),
                            f32::from(inset),
                        ) {
                            schedule(&state, value, &on_change, window, cx);
                        }
                    }
                })
                .on_mouse_up(MouseButton::Left, {
                    let state = state.clone();
                    let on_change = on_change.clone();
                    move |_, window, cx| {
                        cx.stop_propagation();
                        end_drag(&state, &on_change, window, cx);
                    }
                })
                .on_mouse_up_out(MouseButton::Left, {
                    let state = state.clone();
                    let on_change = on_change.clone();
                    move |_, window, cx| end_drag(&state, &on_change, window, cx)
                })
                .on_key_down({
                    let state = state.clone();
                    let range = range.clone();
                    let on_change = on_change.clone();
                    move |event, window, cx| {
                        if event.keystroke.modifiers.modified() {
                            return;
                        }
                        let current = state.read(cx).displayed();
                        let target = match event.keystroke.key.as_str() {
                            "left" | "down" => range.step_value(current, -1),
                            "right" | "up" => range.step_value(current, 1),
                            "home" => range.home_value(),
                            "end" => range.end_value(),
                            _ => return,
                        };
                        window.prevent_default();
                        cx.stop_propagation();
                        state.update(cx, |state, cx| {
                            state.keyboard_active = true;
                            cx.notify();
                        });
                        if target != current {
                            deliver(&state, target, &on_change, window, cx);
                        }
                    }
                });
        }

        // Furniture.
        let readout = self.show_value.then(|| value_text.clone());
        let reset = self.on_reset.clone().map(|handler| {
            let default = range.default;
            (self.reset_label.clone(), handler, default)
        });
        let has_header = self.label.is_some() || readout.is_some() || reset.is_some();
        let hidden = !revealed;
        let header: Option<AnyElement> = has_header.then(|| {
            let mut trailing = div().flex().flex_row().items_center().gap(px(8.));
            if let Some(text) = readout {
                trailing = trailing.child(
                    div()
                        .text_color(tokens.text_muted)
                        .when(hidden, |readout| readout.invisible())
                        .child(text),
                );
            }
            if let Some((text, handler, default)) = reset {
                trailing = trailing.child(
                    div().when(hidden, |reset| reset.invisible()).child(
                        Button::new((self.id.clone(), "reset"))
                            .label(text)
                            .disabled(disabled)
                            .tokens(tokens.clone())
                            .h(px(18.))
                            .px(px(6.))
                            .text_size(px(11.))
                            .on_click(move |_, window, cx| handler(default, window, cx)),
                    ),
                );
            }
            crate::layout::centered_row()
                .justify_between()
                .h(FURNITURE_HEIGHT)
                .w(width)
                .text_size(px(12.))
                .text_color(tokens.text)
                .child(div().children(self.label.clone()))
                .child(trailing)
                .into_any_element()
        });
        let footer: Option<AnyElement> = self.range_labels.clone().map(|(minimum, maximum)| {
            crate::layout::centered_row()
                .justify_between()
                .h(FURNITURE_HEIGHT)
                .w(width)
                .text_size(px(12.))
                .text_color(tokens.text_muted)
                .when(hidden, |footer| footer.invisible())
                .child(minimum)
                .child(maximum)
                .into_any_element()
        });

        div()
            .id(self.id.clone())
            .flex()
            .flex_col()
            .flex_none()
            .w(width)
            .track_focus(&within)
            .on_hover({
                let state = state.clone();
                move |hovered, _, cx| {
                    state.update(cx, |state, cx| {
                        if state.hovered != *hovered {
                            state.hovered = *hovered;
                            if *hovered {
                                state.keyboard_active = false;
                            }
                            cx.notify();
                        }
                    });
                }
            })
            .when(disabled, |root| root.opacity(tokens.disabled_opacity))
            .children(header)
            .child(
                // The press is consumed here, outside the surface: bubbling visits the
                // surface's own listeners (which include pointer capture) before its
                // ancestors', so stopping propagation from this wrapper cannot prevent the
                // capture, yet controls beneath never see the press.
                div()
                    .id((self.id.clone(), "press"))
                    .flex_none()
                    .w(width)
                    .h(SLIDER_HEIGHT)
                    .when(!disabled, |wrapper| {
                        wrapper.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    })
                    .child(surface),
            )
            .children(footer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixels() -> SliderRange {
        SliderRange::new(560.0, 1200.0)
            .with_step(16.0)
            .with_default(736.0)
    }

    const LEFT: f32 = 10.0;
    const WIDTH: f32 = 240.0;
    const INSET: f32 = 7.0;

    fn pointer(range: &SliderRange, x: f32) -> f64 {
        range.value_for_x(x, LEFT, WIDTH, INSET).unwrap()
    }

    #[test]
    fn pointer_endpoints_midpoint_and_steps() {
        let range = pixels();
        // At or left of the first inset: the minimum; at or right of the last: the maximum.
        assert_eq!(pointer(&range, LEFT + INSET), 560.0);
        assert_eq!(pointer(&range, LEFT + INSET - 40.0), 560.0);
        assert_eq!(pointer(&range, LEFT - 500.0), 560.0);
        assert_eq!(pointer(&range, LEFT + WIDTH - INSET), 1200.0);
        assert_eq!(pointer(&range, LEFT + WIDTH - INSET + 40.0), 1200.0);
        assert_eq!(pointer(&range, LEFT + WIDTH + 500.0), 1200.0);
        // The midpoint of the thumb travel.
        assert_eq!(pointer(&range, LEFT + WIDTH / 2.0), 880.0);
        // Every input lands on a multiple of the step above the minimum.
        for index in 0..=600 {
            let x = LEFT - 20.0 + index as f32 * 0.45;
            let value = pointer(&range, x);
            assert!((560.0..=1200.0).contains(&value), "{x}: {value}");
            assert_eq!((value - 560.0) % 16.0, 0.0, "{x}: {value}");
        }
    }

    #[test]
    fn non_finite_input_becomes_the_declared_default() {
        let range = pixels();
        assert_eq!(range.normalize(f64::NAN), 736.0);
        assert_eq!(range.normalize(f64::INFINITY), 736.0);
        assert_eq!(range.normalize(f64::NEG_INFINITY), 736.0);
    }

    #[test]
    fn normalization_clamps_snaps_and_clamps_again() {
        let range = pixels();
        assert_eq!(range.normalize(-1.0e9), 560.0);
        assert_eq!(range.normalize(9.0e9), 1200.0);
        assert_eq!(range.normalize(567.0), 560.0);
        assert_eq!(range.normalize(569.0), 576.0);
        assert_eq!(range.normalize(1199.0), 1200.0);
        // A step that does not divide the range: the snapped value is clamped back.
        let odd = SliderRange::new(0.0, 10.0).with_step(4.0);
        assert_eq!(odd.normalize(9.9), 8.0);
        assert_eq!(odd.normalize(10.0), 10.0, "12 clamped back into the range");
        assert_eq!(odd.normalize(7.0), 8.0);
        // No step: the clamped input itself.
        let continuous = SliderRange::new(0.0, 1.0);
        assert_eq!(continuous.normalize(0.3217), 0.3217);
        assert_eq!(continuous.normalize(2.0), 1.0);
    }

    #[test]
    fn normalization_with_a_stop_list_picks_the_nearest_stop() {
        let range = SliderRange::new(0.0, 100.0).with_stops([0.0, 10.0, 40.0, 100.0]);
        assert_eq!(range.normalize(2.0), 0.0);
        assert_eq!(range.normalize(8.0), 10.0);
        assert_eq!(range.normalize(30.0), 40.0);
        assert_eq!(range.normalize(1.0e6), 100.0);
        assert_eq!(range.normalize(-5.0), 0.0);
        // Equidistant: the earlier stop wins.
        assert_eq!(range.normalize(5.0), 0.0);
        assert_eq!(range.normalize(f64::NAN), range.default);
    }

    #[test]
    fn keyboard_steps_homes_and_clamps_without_wraparound() {
        let range = pixels();
        assert_eq!(range.step_value(736.0, -1), 720.0);
        assert_eq!(range.step_value(736.0, 1), 752.0);
        assert_eq!(range.home_value(), 560.0);
        assert_eq!(range.end_value(), 1200.0);
        // At the ends the value stays: the owner is not notified for an unchanged value.
        assert_eq!(range.step_value(1200.0, 1), 1200.0);
        assert_eq!(range.step_value(560.0, -1), 560.0);
    }

    #[test]
    fn continuous_keyboard_step_is_a_hundredth_of_the_range() {
        let range = SliderRange::new(0.0, 200.0);
        assert_eq!(range.keyboard_step(), 2.0);
        assert_eq!(range.step_value(50.0, 1), 52.0);
        assert_eq!(range.step_value(50.0, -1), 48.0);
        assert_eq!(range.step_value(199.0, 1), 200.0);
    }

    #[test]
    fn keyboard_step_of_a_stop_list_is_one_stop() {
        let range = SliderRange::new(0.0, 100.0).with_stops([0.0, 10.0, 40.0, 100.0]);
        assert_eq!(range.step_value(10.0, 1), 40.0);
        assert_eq!(range.step_value(10.0, -1), 0.0);
        assert_eq!(range.step_value(0.0, -1), 0.0);
        assert_eq!(range.step_value(100.0, 1), 100.0);
        assert_eq!(range.step_value(9.0, 1), 40.0, "from the nearest stop");
    }

    #[test]
    fn five_stops_select_their_index_and_halfway_rounds_up() {
        let inner = WIDTH - 2.0 * INSET;
        for index in 0..5usize {
            let x = LEFT + INSET + (index as f32 / 4.0) * inner;
            assert_eq!(
                stop_index_for_x(x, LEFT, WIDTH, INSET, 5),
                Some(index),
                "stop {index}"
            );
        }
        // Exactly halfway between stops i and i + 1 selects i + 1.
        for index in 0..4usize {
            let x = LEFT + INSET + ((index as f32 + 0.5) / 4.0) * inner;
            assert_eq!(
                stop_index_for_x(x, LEFT, WIDTH, INSET, 5),
                Some(index + 1),
                "halfway above stop {index}"
            );
        }
        // Outside the travel clamps to the end stops.
        assert_eq!(
            stop_index_for_x(LEFT - 50.0, LEFT, WIDTH, INSET, 5),
            Some(0)
        );
        assert_eq!(
            stop_index_for_x(LEFT + WIDTH + 50.0, LEFT, WIDTH, INSET, 5),
            Some(4)
        );
    }

    #[test]
    fn zero_stops_produce_no_index_and_no_value() {
        assert_eq!(stop_index_for_x(50.0, LEFT, WIDTH, INSET, 0), None);
        let range = SliderRange::new(0.0, 1.0).with_stops(Vec::<f64>::new());
        assert_eq!(range.value_for_x(50.0, LEFT, WIDTH, INSET), None);
    }

    #[test]
    fn one_stop_is_always_index_zero_at_fraction_zero() {
        assert_eq!(stop_index_for_x(300.0, LEFT, WIDTH, INSET, 1), Some(0));
        assert_eq!(stop_fraction(0, 1), 0.0);
        assert_eq!(stop_fraction(2, 5), 0.5);
    }

    #[test]
    fn fraction_uses_the_thumb_inset_and_guards_tiny_widths() {
        // Default 14 px thumb: inset 7. Wide thumb: inset 22.
        assert_eq!(fraction_for_x(7.0, 0.0, 240.0, 7.0), 0.0);
        assert_eq!(fraction_for_x(120.0, 0.0, 240.0, 7.0), 0.5);
        assert_eq!(fraction_for_x(22.0, 0.0, 240.0, 22.0), 0.0);
        assert_eq!(fraction_for_x(218.0, 0.0, 240.0, 22.0), 1.0);
        // A surface narrower than twice the inset divides by one, never by zero.
        let value = fraction_for_x(10.0, 0.0, 10.0, 7.0);
        assert!(value.is_finite() && (0.0..=1.0).contains(&value));
    }

    #[test]
    fn geometry_fill_runs_a_rail_radius_past_the_thumb_centre() {
        let geometry = SliderGeometry {
            width: 240.0,
            inset: 7.0,
            rail_height: 4.0,
        };
        assert_eq!(geometry.thumb_center(0.0), 7.0);
        assert_eq!(geometry.thumb_center(1.0), 233.0);
        assert_eq!(geometry.thumb_center(0.5), 120.0);
        assert_eq!(geometry.fill_end(0.0), 9.0);
        assert_eq!(geometry.fill_end(0.5), 122.0);
        assert_eq!(geometry.fill_end(1.0), 235.0);
        // The wide-thumb preset: 44 px thumb, 22 px inset, fill still a rail radius past centre.
        let wide = SliderGeometry {
            width: 240.0,
            inset: 22.0,
            rail_height: 4.0,
        };
        assert_eq!(wide.thumb_center(0.0), 22.0);
        assert_eq!(wide.thumb_center(1.0), 218.0);
        assert_eq!(wide.fill_end(1.0), 220.0);
    }

    #[test]
    fn stops_are_filled_up_to_the_animated_thumb_fraction() {
        let geometry = SliderGeometry {
            width: 240.0,
            inset: 7.0,
            rail_height: 4.0,
        };
        let filled = |fraction: f32| -> Vec<bool> {
            (0..5)
                .map(|index| geometry.stop_filled(index, 5, fraction))
                .collect()
        };
        assert_eq!(filled(0.0), [true, false, false, false, false]);
        // The animated front has not reached the stop at 0.75 yet.
        assert_eq!(filled(0.74), [true, true, true, false, false]);
        assert_eq!(filled(0.75), [true, true, true, true, false]);
        assert_eq!(filled(1.0), [true; 5]);
        // Stops sit at their fractions of the thumb travel, both ends included.
        assert_eq!(geometry.stop_center(0, 5), 7.0);
        assert_eq!(geometry.stop_center(4, 5), 233.0);
        assert_eq!(geometry.stop_center(2, 5), 120.0);
    }

    #[test]
    fn thumb_fraction_follows_the_range_or_the_stop_position() {
        let range = pixels();
        assert_eq!(range.fraction_of(560.0), 0.0);
        assert_eq!(range.fraction_of(880.0), 0.5);
        assert_eq!(range.fraction_of(2000.0), 1.0);
        let stops = SliderRange::new(0.0, 100.0).with_stops([0.0, 10.0, 40.0, 100.0]);
        assert!((stops.fraction_of(40.0) - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(stops.fraction_of(100.0), 1.0);
    }
}
