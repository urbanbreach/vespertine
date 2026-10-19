//! Phase-aware bounded elastic overscroll (CMP-030).
//!
//! [`elastic_scroll`] wraps a vertically scrolling container (a [`ScrollHandle`] or a virtual
//! [`ListState`]) in a layout-transparent element. The container's own offset stays clamped
//! exactly as before; the stretch is a visual vertical displacement of the wrapped content,
//! clipped to the wrapper's bounds and driven by touch-phase-aware precise (pixel) scroll input.
//!
//! The pure parts are public so they can be tested without a window: [`rubber_band`],
//! [`rubber_band_inverse`], [`ReturnSpring`] and the gesture state machine [`ElasticState`].
//!
//! On Linux the base backends deliver only the `Moved` phase, so the wrapper is disabled by
//! default and scrolling is ordinary clamped scrolling; a host or backend with reliable
//! `Started`/`Ended` phases enables it with [`ReliableScrollPhases`] (or per wrapper with
//! [`ElasticScroll::reliable_phases`]). Phases are never inferred from timing.

use std::{cell::RefCell, rc::Rc};

use vespertine::{
    AnyElement, App, Bounds, ContentMask, DispatchPhase, Element, ElementId, Global,
    GlobalElementId, Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, ListState,
    MotionClock, Pixels, ScrollDelta, ScrollHandle, ScrollWheelEvent, TouchPhase, Window, point,
    px,
};

/// The rubber-band coefficient (R4).
pub const BAND_COEFFICIENT: f32 = 0.55;
/// The return spring's angular frequency per second (R5).
pub const SPRING_OMEGA: f32 = 13.0;
/// The spring ends below this offset in px (R5).
pub const SPRING_STOP_OFFSET: f32 = 0.25;
/// The spring ends below this velocity in px/s (R5).
pub const SPRING_STOP_VELOCITY: f32 = 10.0;
/// Silence beyond this resets inferred momentum, in milliseconds (R6).
pub const FLING_GAP_MS: f64 = 120.0;
/// The momentum bounce peak is `min(0.12 d, 80 px)` (R6).
pub const BOUNCE_PEAK_FRACTION: f32 = 0.12;
/// Cap of the momentum bounce peak in px (R6).
pub const BOUNCE_PEAK_CAP: f32 = 80.0;

fn signum(value: f32) -> f32 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// The displayed offset for finger travel `distance` past an end, for a viewport extent `extent`
/// (minimum 1): `d x (1 - 1 / (1 + 0.55 x / d))`, odd, strictly increasing, below both the travel
/// and the extent (R4).
pub fn rubber_band(distance: f32, extent: f32) -> f32 {
    let d = extent.max(1.0);
    let x = distance.abs();
    signum(distance) * d * (1.0 - 1.0 / (1.0 + BAND_COEFFICIENT * x / d))
}

/// The travel for a displayed offset `displayed` (caught mid-return): `(d / 0.55) x (1 / (1 -
/// s / d) - 1)` with the displayed magnitude limited to `0.99 d` (R4).
pub fn rubber_band_inverse(displayed: f32, extent: f32) -> f32 {
    let d = extent.max(1.0);
    let s = displayed.abs().min(0.99 * d);
    signum(displayed) * (d / BAND_COEFFICIENT) * (1.0 / (1.0 - s / d) - 1.0)
}

/// The critically damped return spring of R5 from offset `x0` with velocity `v0` (px/s).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReturnSpring {
    /// The initial offset.
    pub x0: f32,
    /// The initial velocity in px/s.
    pub v0: f32,
    /// The start time in milliseconds.
    pub start_ms: f64,
}

impl ReturnSpring {
    /// The offset and velocity `t` seconds after the start:
    /// `x(t) = (x0 + (v0 + 13 x0) t) e^(-13 t)`.
    pub fn sample(&self, t: f32) -> (f32, f32) {
        let b = self.v0 + SPRING_OMEGA * self.x0;
        let decay = (-SPRING_OMEGA * t).exp();
        let x = (self.x0 + b * t) * decay;
        let v = (b - SPRING_OMEGA * (self.x0 + b * t)) * decay;
        (x, v)
    }
}

/// The room an event has toward its direction (R3). Positive deltas scroll toward the top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    /// Pixels the container can still scroll toward the top.
    pub top: f32,
    /// Pixels the container can still scroll toward the bottom.
    pub bottom: f32,
}

impl Room {
    /// Unbounded room: the end is not on screen (or an event outside the region).
    pub const UNBOUNDED: Room = Room {
        top: f32::INFINITY,
        bottom: f32::INFINITY,
    };

    fn toward(&self, delta: f32) -> f32 {
        if delta > 0.0 { self.top } else { self.bottom }
    }
}

/// The gesture state machine of one wrapper (R6), retained across frames.
#[derive(Clone, Debug)]
pub struct ElasticState {
    extent: f32,
    offset: f32,
    stretch: f32,
    touching: bool,
    touch_moved: bool,
    momentum: bool,
    fling_bounced: bool,
    fling_velocity: f32,
    last_event_ms: Option<f64>,
    spring: Option<ReturnSpring>,
    springs_created: u32,
}

impl Default for ElasticState {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl ElasticState {
    /// A state for a wrapper `extent` px tall.
    pub fn new(extent: f32) -> Self {
        Self {
            extent: extent.max(1.0),
            offset: 0.0,
            stretch: 0.0,
            touching: false,
            touch_moved: false,
            momentum: false,
            fling_bounced: false,
            fling_velocity: 0.0,
            last_event_ms: None,
            spring: None,
            springs_created: 0,
        }
    }

    /// Sets the viewport extent `d` (the wrapper's bounds height, minimum 1).
    pub fn set_extent(&mut self, extent: f32) {
        self.extent = extent.max(1.0);
    }

    /// The displayed band offset (positive at the top end, negative at the bottom end).
    pub fn offset(&self) -> f32 {
        self.offset
    }

    /// Whether a return spring runs.
    pub fn spring_active(&self) -> bool {
        self.spring.is_some()
    }

    /// Whether a touch is in progress.
    pub fn touching(&self) -> bool {
        self.touching
    }

    /// Whether the inferred momentum phase of a fling is active.
    pub fn momentum(&self) -> bool {
        self.momentum
    }

    /// How many return springs this state has created (a test observation).
    pub fn springs_created(&self) -> u32 {
        self.springs_created
    }

    /// Drops everything (the wrapper is inert): offset 0, no spring.
    pub fn reset(&mut self) {
        let extent = self.extent;
        let created = self.springs_created;
        *self = Self::new(extent);
        self.springs_created = created;
    }

    fn start_spring(&mut self, x0: f32, v0: f32, now_ms: f64) {
        self.spring = Some(ReturnSpring {
            x0,
            v0,
            start_ms: now_ms,
        });
        self.springs_created += 1;
    }

    fn release(&mut self, now_ms: f64) {
        self.touching = false;
        self.touch_moved = false;
        self.stretch = 0.0;
        self.fling_velocity = 0.0;
        self.fling_bounced = self.offset != 0.0;
        if self.offset != 0.0 && self.spring.is_none() {
            self.start_spring(self.offset, 0.0, now_ms);
        }
    }

    /// Feeds one scroll-wheel event: its phase, delta `(dx, dy)` in px, the container's room and
    /// the event time. Returns whether the event is consumed (must not reach the container)
    /// (R2, R6). Line deltas are not fed; a delta whose horizontal magnitude exceeds its vertical
    /// one contributes no vertical movement.
    pub fn wheel(&mut self, phase: TouchPhase, delta: (f32, f32), room: Room, now_ms: f64) -> bool {
        let (dx, dy) = delta;
        let vertical = if dx.abs() > dy.abs() { 0.0 } else { dy };
        let consumed = match phase {
            TouchPhase::Started => {
                self.touching = true;
                self.touch_moved = false;
                self.momentum = false;
                self.fling_bounced = false;
                false
            }
            TouchPhase::Cancelled => {
                self.release(now_ms);
                false
            }
            TouchPhase::Moved | TouchPhase::Ended => {
                if phase == TouchPhase::Moved
                    && self.touching
                    && !self.touch_moved
                    && dx == 0.0
                    && dy == 0.0
                {
                    // A touch that never scrolled (a cancelled touch looks like this when
                    // cancellation is not mapped).
                    self.release(now_ms);
                    self.last_event_ms = Some(now_ms);
                    return false;
                }
                let silent = self
                    .last_event_ms
                    .is_none_or(|last| now_ms - last > FLING_GAP_MS);
                if !self.touching && silent {
                    self.momentum = false;
                    self.fling_bounced = false;
                    self.fling_velocity = 0.0;
                }
                let consumed = if self.touching {
                    self.drag(vertical, room)
                } else {
                    self.free(vertical, room, now_ms)
                };
                if phase == TouchPhase::Ended {
                    self.release(now_ms);
                    self.momentum = true;
                }
                consumed
            }
        };
        self.last_event_ms = Some(now_ms);
        consumed
    }

    fn drag(&mut self, delta: f32, room: Room) -> bool {
        if delta == 0.0 {
            return false;
        }
        self.touch_moved = true;
        if self.stretch == 0.0 && self.offset != 0.0 {
            // The finger caught the band mid-return.
            self.stretch = rubber_band_inverse(self.offset, self.extent);
        }
        self.spring = None;
        if self.stretch != 0.0 {
            let inward = signum(delta) != signum(self.stretch);
            let next = self.stretch + delta;
            if signum(next) == signum(self.stretch) {
                self.stretch = next;
                self.offset = rubber_band(next, self.extent);
                inward
            } else {
                // A delta larger than the stretch: the whole delta belongs to the container.
                self.stretch = 0.0;
                self.offset = 0.0;
                false
            }
        } else {
            let past = delta.abs() - room.toward(delta);
            if past > 0.0 {
                self.stretch = past * signum(delta);
                self.offset = rubber_band(self.stretch, self.extent);
            }
            false
        }
    }

    fn free(&mut self, delta: f32, room: Room, now_ms: f64) -> bool {
        if delta == 0.0 {
            return false;
        }
        if self.offset != 0.0 || self.spring.is_some() {
            if self.offset == 0.0 {
                // Only a spring without offset remains: momentum is spent against the end.
                return false;
            }
            if signum(delta) == signum(self.offset) {
                // Deepens or maintains the stretch: ignored while it settles.
                return false;
            }
            // An inward delta unwinds the settling band as a drag would.
            let stretch = rubber_band_inverse(self.offset, self.extent);
            let next = stretch + delta;
            if signum(next) != signum(stretch) {
                self.offset = 0.0;
                self.stretch = 0.0;
                self.spring = None;
                return false;
            }
            self.offset = rubber_band(next, self.extent);
            self.start_spring(self.offset, 0.0, now_ms);
            return true;
        }
        // The band is at rest.
        let dt = self
            .last_event_ms
            .map_or(1.0 / 60.0, |last| ((now_ms - last) / 1000.0) as f32)
            .clamp(1.0 / 240.0, 1.0 / 30.0);
        let estimate = delta / dt;
        self.fling_velocity =
            if self.fling_velocity != 0.0 && signum(self.fling_velocity) == signum(estimate) {
                0.5 * (self.fling_velocity + estimate)
            } else {
                estimate
            };
        if self.momentum && !self.fling_bounced && delta.abs() > room.toward(delta) {
            self.fling_bounced = true;
            let peak = (BOUNCE_PEAK_FRACTION * self.extent).min(BOUNCE_PEAK_CAP);
            let max_velocity = peak * SPRING_OMEGA * std::f32::consts::E;
            let velocity = self.fling_velocity.clamp(-max_velocity, max_velocity);
            self.start_spring(0.0, velocity, now_ms);
        }
        false
    }

    /// Steps the return spring once per frame (R5): ends it at offset 0 below 0.25 px and
    /// 10 px/s. Returns whether the state changed.
    pub fn tick(&mut self, now_ms: f64) -> bool {
        let Some(spring) = self.spring else {
            return false;
        };
        let t = ((now_ms - spring.start_ms) / 1000.0).max(0.0) as f32;
        let (x, v) = spring.sample(t);
        if x.abs() < SPRING_STOP_OFFSET && v.abs() < SPRING_STOP_VELOCITY {
            self.spring = None;
            self.offset = 0.0;
        } else {
            self.offset = x;
        }
        true
    }
}

/// The host capability flag of R9: whether the platform input delivers reliable
/// `Started`/`Ended` phases for finger scrolling. Unset, the wrappers are inert.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReliableScrollPhases(pub bool);

impl Global for ReliableScrollPhases {}

/// What a wrapper scrolls (R1).
#[derive(Clone, Debug)]
pub enum ScrollSource {
    /// A plain scroll container tracked by a handle.
    Handle(ScrollHandle),
    /// A virtual list.
    List(ListState),
}

impl ScrollSource {
    /// The room toward the top and the bottom (R3). Room is never negative.
    pub fn room(&self) -> Room {
        match self {
            ScrollSource::Handle(handle) => {
                let scrolled = -f32::from(handle.offset().y);
                let max = f32::from(handle.max_offset().y);
                Room {
                    top: scrolled.max(0.0),
                    bottom: (max - scrolled).max(0.0),
                }
            }
            ScrollSource::List(list) => {
                let count = list.item_count();
                if count == 0 {
                    return Room {
                        top: 0.0,
                        bottom: 0.0,
                    };
                }
                let top = (-f32::from(list.scroll_px_offset_for_scrollbar().y)).max(0.0);
                let viewport = list.viewport_bounds();
                let bottom = match list.bounds_for_item(count - 1) {
                    Some(last) => f32::from(last.bottom() - viewport.bottom()).max(0.0),
                    None => f32::INFINITY,
                };
                Room { top, bottom }
            }
        }
    }
}

/// The layout-transparent elastic wrapper (R1).
pub struct ElasticScroll {
    id: ElementId,
    source: ScrollSource,
    child: AnyElement,
    enabled: bool,
    reliable_phases: Option<bool>,
}

/// Wraps `child`, the scroll container that owns `source`, in an elastic overscroll wrapper.
pub fn elastic_scroll(
    id: impl Into<ElementId>,
    source: ScrollSource,
    child: impl IntoElement,
) -> ElasticScroll {
    ElasticScroll {
        id: id.into(),
        source,
        child: child.into_any_element(),
        enabled: true,
        reliable_phases: None,
    }
}

impl ElasticScroll {
    /// Whether the host enables the wrapper (R8). Enabled by default.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The per-wrapper capability override: this wrapper's input source delivers reliable
    /// gesture phases (a test override; hosts set [`ReliableScrollPhases`] globally).
    pub fn reliable_phases(mut self, reliable: bool) -> Self {
        self.reliable_phases = Some(reliable);
        self
    }

    fn is_active(&self, cx: &App) -> bool {
        let reliable = self.reliable_phases.unwrap_or_else(|| {
            cx.try_global::<ReliableScrollPhases>()
                .is_some_and(|flag| flag.0)
        });
        self.enabled && reliable && !cx.reduce_motion() && !cx.has_active_drag()
    }
}

/// What a wrapper prepaints.
pub struct ElasticPrepaint {
    state: Rc<RefCell<ElasticState>>,
    hitbox: Option<Hitbox>,
    offset: f32,
}

impl Element for ElasticScroll {
    type RequestLayoutState = ();
    type PrepaintState = ElasticPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
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
    ) -> (LayoutId, ()) {
        // Layout passes straight through to the child: the wrapper adds no box.
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> ElasticPrepaint {
        let active = self.is_active(cx);
        let now = MotionClock::now_ms(cx);
        let state = window.with_element_state(
            id.expect("the elastic wrapper has an id"),
            |state: Option<Rc<RefCell<ElasticState>>>, _| {
                let state = state.unwrap_or_default();
                (state.clone(), state)
            },
        );
        let offset = {
            let mut state = state.borrow_mut();
            state.set_extent(f32::from(bounds.size.height));
            if active {
                state.tick(now);
            } else {
                // Inert: all state is reset every frame (disabling mid-gesture drops the
                // stretch immediately).
                state.reset();
            }
            if state.spring_active() {
                window.request_animation_frame();
            }
            state.offset()
        };
        let hitbox = active.then(|| window.insert_hitbox(bounds, HitboxBehavior::Normal));
        if offset == 0.0 {
            self.child.prepaint(window, cx);
        } else {
            // The band displaces the child's prepaint and paint origin under a content mask
            // equal to the wrapper's bounds.
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                window.with_element_offset(point(px(0.0), px(offset)), |window| {
                    self.child.prepaint(window, cx);
                });
            });
        }
        ElasticPrepaint {
            state,
            hitbox,
            offset,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        prepaint: &mut ElasticPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(hitbox) = prepaint.hitbox.clone() {
            let state = prepaint.state.clone();
            let source = self.source.clone();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                // The capture phase runs before the container's own listener.
                if phase != DispatchPhase::Capture {
                    return;
                }
                let ScrollDelta::Pixels(delta) = event.delta else {
                    return;
                };
                let now = MotionClock::now_ms(cx);
                let consumed = if hitbox.is_hovered(window) {
                    let room = source.room();
                    state.borrow_mut().wheel(
                        event.touch_phase,
                        (f32::from(delta.x), f32::from(delta.y)),
                        room,
                        now,
                    )
                } else if event.touch_phase != TouchPhase::Moved {
                    // Outside the region the gesture state still advances, but never moves
                    // the band.
                    state
                        .borrow_mut()
                        .wheel(event.touch_phase, (0.0, 0.0), Room::UNBOUNDED, now)
                } else {
                    return;
                };
                window.refresh();
                if consumed {
                    cx.stop_propagation();
                }
            });
        }
        if prepaint.offset == 0.0 {
            self.child.paint(window, cx);
        } else {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                self.child.paint(window, cx);
            });
        }
    }
}

impl IntoElement for ElasticScroll {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: f32 = 400.0;
    const TOP: Room = Room {
        top: 0.0,
        bottom: 1000.0,
    };

    #[test]
    fn a2_the_band_is_odd_increasing_and_bounded_and_the_inverse_recovers_the_travel() {
        let mut previous = 0.0;
        let mut x = 20.0;
        while x <= 1000.0 {
            let y = rubber_band(x, D);
            assert!(y > previous, "strictly increasing at {x}");
            assert!(y < x && y < D, "below the travel and the extent at {x}");
            assert!(
                (rubber_band_inverse(y, D) - x).abs() < 0.5,
                "inverse at {x}"
            );
            assert_eq!(rubber_band(-x, D), -y, "odd at {x}");
            previous = y;
            x += 20.0;
        }
        assert!((rubber_band(60.0, 320.0) - 29.9).abs() < 0.1);
        assert!((rubber_band(60.0, 400.0) - 30.5).abs() < 0.1);
        assert_eq!(rubber_band(0.0, D), 0.0);
        // The extent is at least one pixel.
        assert!(rubber_band(10.0, 0.0).is_finite());
        assert!(rubber_band_inverse(10_000.0, D).is_finite());
    }

    #[test]
    fn a3_reverse_deltas_unwind_the_band_and_a_large_one_reaches_the_container() {
        let mut state = ElasticState::new(D);
        assert!(!state.wheel(TouchPhase::Started, (0.0, 0.0), TOP, 0.0));
        assert!(!state.wheel(TouchPhase::Moved, (0.0, 60.0), TOP, 8.0));
        let offset = state.offset();
        assert!(offset > 0.0 && offset < 60.0, "offset {offset}");
        // An inward delta is consumed and reduces the offset.
        assert!(state.wheel(TouchPhase::Moved, (0.0, -20.0), TOP, 16.0));
        assert!(state.offset() < offset && state.offset() > 0.0);
        // An inward delta larger than the remaining band collapses it and is NOT consumed.
        assert!(!state.wheel(TouchPhase::Moved, (0.0, -100.0), TOP, 24.0));
        assert_eq!(state.offset(), 0.0);
        // At rest, inward travel is not consumed.
        assert!(!state.wheel(TouchPhase::Moved, (0.0, -20.0), TOP, 32.0));
        assert_eq!(state.offset(), 0.0);
    }

    #[test]
    fn a4_a_phase_less_stream_never_bounces() {
        let mut state = ElasticState::new(D);
        let mut now = 0.0;
        for i in 0..40 {
            now += 8.0;
            let delta = if i % 2 == 0 { -60.0 } else { 60.0 };
            let consumed = state.wheel(TouchPhase::Moved, (0.0, delta), TOP, now);
            assert!(!consumed);
            assert!(!state.spring_active());
            assert_eq!(state.offset(), 0.0);
        }
        assert_eq!(state.springs_created(), 0);
    }

    #[test]
    fn a5_a_touch_fling_creates_exactly_one_bounded_bounce_that_settles() {
        let mut state = ElasticState::new(D);
        state.wheel(
            TouchPhase::Started,
            (0.0, 0.0),
            Room {
                top: 50.0,
                bottom: 1000.0,
            },
            0.0,
        );
        state.wheel(
            TouchPhase::Moved,
            (0.0, 20.0),
            Room {
                top: 50.0,
                bottom: 1000.0,
            },
            8.0,
        );
        assert_eq!(state.offset(), 0.0, "inside the room nothing stretches");
        state.wheel(
            TouchPhase::Ended,
            (0.0, 0.0),
            Room {
                top: 30.0,
                bottom: 1000.0,
            },
            16.0,
        );
        assert!(state.momentum());
        // Momentum events at the top: the first reaches the end and bounces.
        let mut now = 16.0;
        let mut peak = 0.0f32;
        for _ in 0..3 {
            now += 8.0;
            state.wheel(
                TouchPhase::Moved,
                (0.0, 40.0),
                Room {
                    top: 0.0,
                    bottom: 1000.0,
                },
                now,
            );
        }
        assert_eq!(state.springs_created(), 1, "exactly one spring");
        let mut t = now;
        while state.spring_active() && t < now + 2000.0 {
            t += 4.0;
            state.tick(t);
            assert!(state.offset() >= 0.0, "never negative");
            peak = peak.max(state.offset());
        }
        assert!(peak > 10.0, "peak {peak}");
        assert!(peak <= 0.12 * 400.0 + 0.5, "peak {peak}");
        assert!(!state.spring_active());
        assert_eq!(state.offset(), 0.0);
        assert!(t - now <= 2000.0);
    }

    #[test]
    fn a6_a_zero_delta_touch_is_a_cancel_and_horizontal_travel_makes_no_band() {
        let mut state = ElasticState::new(D);
        state.wheel(TouchPhase::Started, (0.0, 0.0), TOP, 0.0);
        // Put a stretch on the band first by a second gesture? No: a fresh touch that never
        // scrolled is released.
        assert!(!state.wheel(TouchPhase::Moved, (0.0, 0.0), TOP, 8.0));
        assert!(!state.touching());
        // With a nonzero offset a spring runs after the cancel.
        let mut state = ElasticState::new(D);
        state.wheel(TouchPhase::Started, (0.0, 0.0), TOP, 0.0);
        state.wheel(TouchPhase::Moved, (0.0, 80.0), TOP, 8.0);
        assert!(state.offset() > 0.0);
        state.wheel(TouchPhase::Cancelled, (0.0, 0.0), TOP, 16.0);
        assert!(!state.touching() && state.spring_active());
        // A dominantly horizontal delta contributes no vertical movement.
        let mut state = ElasticState::new(D);
        state.wheel(TouchPhase::Started, (0.0, 0.0), TOP, 0.0);
        assert!(!state.wheel(TouchPhase::Moved, (-50.0, 20.0), TOP, 8.0));
        assert_eq!(state.offset(), 0.0);
    }

    #[test]
    fn a8_the_critically_damped_return_matches_the_closed_form() {
        let spring = ReturnSpring {
            x0: 80.0,
            v0: 0.0,
            start_ms: 0.0,
        };
        let (x, _) = spring.sample(0.05);
        assert!((x - 68.9).abs() < 0.1, "x(0.05) = {x}");
        let (x, _) = spring.sample(0.45);
        assert!((x - 1.6).abs() < 0.1, "x(0.45) = {x}");
        let mut t = 0.0;
        while t < 2.0 {
            assert!(spring.sample(t).0 >= 0.0);
            t += 0.01;
        }
        // With v0 > 0 and x0 = 0 the peak is v0 / (13 e) at t = 1/13 s.
        let v0 = 1000.0;
        let spring = ReturnSpring {
            x0: 0.0,
            v0,
            start_ms: 0.0,
        };
        let peak = v0 / (SPRING_OMEGA * std::f32::consts::E);
        let (x, v) = spring.sample(1.0 / 13.0);
        assert!((x - peak).abs() < 0.05, "{x} vs {peak}");
        assert!(v.abs() < 0.5);
        let mut max = 0.0f32;
        let mut t = 0.0;
        while t < 1.0 {
            max = max.max(spring.sample(t).0);
            t += 0.001;
        }
        assert!((max - peak).abs() < 0.1);
    }

    #[test]
    fn momentum_unwinds_an_inward_band_and_ignores_deepening_input() {
        let mut state = ElasticState::new(D);
        state.wheel(TouchPhase::Started, (0.0, 0.0), TOP, 0.0);
        state.wheel(TouchPhase::Moved, (0.0, 90.0), TOP, 8.0);
        state.wheel(TouchPhase::Ended, (0.0, 0.0), TOP, 16.0);
        assert!(state.spring_active());
        // Momentum that deepens the stretch is ignored while it settles.
        assert!(!state.wheel(TouchPhase::Moved, (0.0, 30.0), TOP, 24.0));
        let before = state.offset();
        // An inward delta unwinds it and is consumed.
        assert!(state.wheel(TouchPhase::Moved, (0.0, -10.0), TOP, 32.0));
        assert!(state.offset() < before);
        assert!(state.spring_active());
    }

    #[test]
    fn an_inert_state_resets_everything() {
        let mut state = ElasticState::new(D);
        state.wheel(TouchPhase::Started, (0.0, 0.0), TOP, 0.0);
        state.wheel(TouchPhase::Moved, (0.0, 90.0), TOP, 8.0);
        assert!(state.offset() > 0.0);
        state.reset();
        assert_eq!(state.offset(), 0.0);
        assert!(!state.touching() && !state.spring_active());
    }
}
