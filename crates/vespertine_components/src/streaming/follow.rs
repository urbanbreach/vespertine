//! The pure parts of the follow policy (CMP-029 R1-R7, R9, R10, R14, R21): pinned versus
//! detached, the velocity-spring stepper, long jumps, reservations, selection autoscroll and the
//! navigation timeline.

/// Distance (px) from the end within which the view counts as exactly at the bottom (R3, R4).
pub const AT_BOTTOM_EPSILON: f32 = 2.0;
/// Distance within which moving toward the bottom re-pins (R4).
pub const REENGAGE_BAND: f32 = 70.0;
/// A user scroll breaks the pin only when the distance grew by more than this (R3).
pub const BREAK_THRESHOLD: f32 = 1.0;
/// The jump-to-latest affordance is offered beyond this distance (R5).
pub const JUMP_SHOW_DISTANCE: f32 = 320.0;
/// Distances beyond this many viewports first teleport the excess (R5, R9).
pub const TELEPORT_VIEWPORTS: f32 = 2.5;
/// The glide lands exactly within this distance (R6).
pub const LANDING_SNAP: f32 = 0.5;
/// The length of one 60 fps frame in milliseconds.
pub const FRAME_MS: f64 = 1000.0 / 60.0;
/// A hitch catches up at most this many frames (R6).
pub const CATCH_UP_FRAMES: f64 = 8.0;
/// Spring state is retained this long after landing (R7).
pub const SETTLE_GRACE_MS: f64 = 500.0;
/// Reservation changes below this are jitter (R10).
pub const RESERVATION_JITTER: f32 = 0.5;

/// What a user scroll event did to the pin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinChange {
    /// The pin is unchanged.
    Unchanged,
    /// The pin broke (R3): the spring and its timers reset.
    Broke,
    /// The pin re-engaged (R4): the follow spring wakes.
    Repinned,
}

/// Pinned-versus-detached bookkeeping (R1-R5). The intent is derived only from user scroll
/// events; position changes caused by the system never reach [`FollowPolicy::on_user_scroll`].
#[derive(Clone, Debug)]
pub struct FollowPolicy {
    pinned: bool,
    previous_distance: f32,
    jump_visible: bool,
}

impl Default for FollowPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl FollowPolicy {
    /// A pinned policy at the bottom.
    pub fn new() -> Self {
        Self {
            pinned: true,
            previous_distance: 0.0,
            jump_visible: false,
        }
    }

    /// Whether the view follows the end.
    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// Pins or unpins directly (programmatic: explicit navigation, jump to latest).
    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
    }

    /// The previous distance from the bottom, recorded at every scroll event and after
    /// programmatic landings.
    pub fn previous_distance(&self) -> f32 {
        self.previous_distance
    }

    /// Records the distance (after a programmatic landing).
    pub fn record_distance(&mut self, distance: f32) {
        self.previous_distance = distance;
    }

    /// A user scroll event with the new distance `distance` from the bottom (R3, R4).
    ///
    /// Breaks the pin when `D > previous + 1` and `D > 2`; re-pins when `D <= 2` (exactly at the
    /// bottom) or when `D <= 70` and `D < previous` (strictly moving toward the bottom: a small
    /// wheel-up notch inside the band must not snap straight back).
    pub fn on_user_scroll(&mut self, distance: f32) -> PinChange {
        let previous = self.previous_distance;
        self.previous_distance = distance;
        let change = if distance > previous + BREAK_THRESHOLD && distance > AT_BOTTOM_EPSILON {
            if self.pinned {
                self.pinned = false;
                PinChange::Broke
            } else {
                PinChange::Unchanged
            }
        } else if distance <= AT_BOTTOM_EPSILON
            || (distance <= REENGAGE_BAND && distance < previous)
        {
            if self.pinned {
                PinChange::Unchanged
            } else {
                self.pinned = true;
                PinChange::Repinned
            }
        } else {
            PinChange::Unchanged
        };
        self.update_jump_button(distance);
        change
    }

    /// The jump-to-latest visibility with hysteresis (R5): offered when unpinned and
    /// `D > 320`; once offered it stays until `D <= 2`.
    pub fn update_jump_button(&mut self, distance: f32) -> bool {
        if distance <= AT_BOTTOM_EPSILON || self.pinned && !self.jump_visible {
            self.jump_visible = false;
        } else if !self.pinned && distance > JUMP_SHOW_DISTANCE {
            self.jump_visible = true;
        }
        self.jump_visible
    }

    /// Whether the jump-to-latest affordance is shown.
    pub fn show_jump_button(&self) -> bool {
        self.jump_visible
    }

    /// Hides the affordance (activating it re-pins).
    pub fn hide_jump_button(&mut self) {
        self.jump_visible = false;
    }
}

/// The px a pinned stepper (or a jump to latest) first teleports toward the end when `distance`
/// exceeds 2.5 viewports (R5, R9); zero otherwise and for a non-positive viewport.
pub fn teleport_excess(distance: f32, viewport: f32) -> f32 {
    if viewport > 0.0 && distance > TELEPORT_VIEWPORTS * viewport {
        distance - TELEPORT_VIEWPORTS * viewport
    } else {
        0.0
    }
}

/// The follow stepper (R6, R7): a velocity spring with a growth feed-forward.
///
/// Positions are scroll offsets in px (larger = closer to the bottom). Parameters: damping 0.7,
/// stiffness 0.05, mass 1.25, growth average rate 0.12, chase lead `min(9 g, 32)`.
#[derive(Clone, Debug, Default)]
pub struct Stepper {
    velocity: f32,
    growth: f32,
    last_target: Option<f32>,
    last_tick_ms: Option<f64>,
    settled_at_ms: Option<f64>,
}

impl Stepper {
    /// A cold stepper.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets all spring state (the next tick is a cold start).
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The growth estimate `g` (px per frame).
    pub fn growth(&self) -> f32 {
        self.growth
    }

    /// The velocity `v` (px per 60 fps frame).
    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    /// One pure step (R6): advances `position` toward `target` by `frames` 60 fps frames.
    ///
    /// Never overshoots, is monotone while approaching and snaps to the target once within 0.5 px.
    pub fn step(&mut self, position: f32, target: f32, frames: f32) -> f32 {
        let last_target = self.last_target.unwrap_or(target);
        let grew = target - last_target;
        if grew < -1.0 {
            self.growth = 0.0;
        } else {
            let observed = grew.max(0.0) / frames.max(0.25);
            self.growth += 0.12 * (observed - self.growth);
        }
        self.last_target = Some(target);
        let chase = target - (9.0 * self.growth).min(32.0);
        let mut position = position;
        let mut remaining = frames;
        while remaining > 0.0 {
            let h = remaining.min(1.0);
            let diff = (chase - position).max(0.0);
            self.velocity += h * ((0.7 * self.velocity + 0.05 * diff) / 1.25 - self.velocity);
            position = (position + (self.velocity + self.growth) * h).min(target);
            remaining -= h;
        }
        if target - position <= LANDING_SNAP {
            position = target;
        }
        position
    }

    /// Whether the stepper needs no further frames at this position: it is within the landing
    /// distance of the target (R7: a stationary spring must not keep repainting).
    pub fn is_idle(&self, position: f32, target: f32) -> bool {
        target - position <= LANDING_SNAP
    }

    /// One timed tick: derives the frame count from the elapsed time (capped at 8, 1 for the
    /// first tick after a cold start), resets cold after the 500 ms settle grace and steps.
    pub fn tick(&mut self, now_ms: f64, position: f32, target: f32) -> f32 {
        if let Some(settled) = self.settled_at_ms
            && now_ms - settled > SETTLE_GRACE_MS
        {
            self.reset();
        }
        let frames = match self.last_tick_ms {
            Some(last) => ((now_ms - last) / FRAME_MS).clamp(0.0, CATCH_UP_FRAMES) as f32,
            None => 1.0,
        };
        self.last_tick_ms = Some(now_ms);
        let next = self.step(position, target, frames);
        if self.is_idle(next, target) {
            self.settled_at_ms.get_or_insert(now_ms);
        } else {
            self.settled_at_ms = None;
        }
        next
    }
}

/// The reserved bottom chrome height (R10): `clamp(requested, 0, limit)`.
pub fn reserved_height(requested: f32, limit: f32) -> f32 {
    requested.clamp(0.0, limit.max(0.0))
}

/// The list viewport height for a parent height and a reservation (R10), clamped at 0.
pub fn list_viewport_height(parent: f32, reserved: f32) -> f32 {
    (parent - reserved).max(0.0)
}

/// Whether a reservation change counts (more than 0.5 px; sub-pixel jitter is ignored) (R10).
pub fn reservation_changed(old: f32, new: f32) -> bool {
    (old - new).abs() > RESERVATION_JITTER
}

/// The step per autoscroll tick for a pointer at `pointer_y` over a viewport spanning `top` to
/// `bottom` (R14): the edge zone is `min(36, height / 3)`; with penetration `p` from the zone's
/// inner boundary toward (and beyond) the viewport edge, `t = clamp(p / zone, 0, 1)` and the step
/// is `24 x t^2`, negative toward the top and positive toward the bottom, zero elsewhere.
pub fn autoscroll_step(pointer_y: f32, top: f32, bottom: f32) -> f32 {
    let height = bottom - top;
    if height <= 0.0 {
        return 0.0;
    }
    let zone = 36.0f32.min(height / 3.0);
    if pointer_y < top + zone {
        let t = ((top + zone - pointer_y) / zone).clamp(0.0, 1.0);
        -24.0 * t * t
    } else if pointer_y > bottom - zone {
        let t = ((pointer_y - (bottom - zone)) / zone).clamp(0.0, 1.0);
        24.0 * t * t
    } else {
        0.0
    }
}

/// The autoscroll tick period in milliseconds (R14).
pub const AUTOSCROLL_TICK_MS: u64 = 24;

/// Cubic bezier easing through (0,0) and (1,1) with control points `(x1, y1)`, `(x2, y2)`.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let sample = |a: f32, b: f32, s: f32| {
        // B(s) with endpoints 0 and 1.
        let inv = 1.0 - s;
        3.0 * inv * inv * s * a + 3.0 * inv * s * s * b + s * s * s
    };
    let mut lo = 0.0f32;
    let mut hi = 1.0f32;
    let mut s = t;
    for _ in 0..40 {
        let x = sample(x1, x2, s);
        if (x - t).abs() < 1e-6 {
            break;
        }
        if x < t {
            lo = s;
        } else {
            hi = s;
        }
        s = 0.5 * (lo + hi);
    }
    sample(y1, y2, s)
}

/// The navigation ease-in-out of R21: cubic-bezier(0.42, 0, 0.58, 1).
pub fn navigation_ease(progress: f32) -> f32 {
    cubic_bezier(0.42, 0.0, 0.58, 1.0, progress)
}

/// The fixed-duration navigation timeline of R21.
///
/// Each frame moves by the fraction of the CURRENT remaining distance
/// `(e_now - e_prev) / (1 - e_prev)`, which telescopes to exactly `start + e(t) x total` for a
/// stable estimate and continues on the same timeline over a corrected remainder when estimates
/// change (never percent-of-remaining).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavigationTimeline {
    start_ms: f64,
    duration_ms: f64,
    previous_eased: f32,
}

/// The base duration of a navigation (R21), before the global motion speed factor.
pub const NAVIGATION_DURATION_MS: f64 = 500.0;

impl NavigationTimeline {
    /// A timeline starting at `start_ms` lasting `duration_ms` (scaled by the global motion speed
    /// factor by the caller).
    pub fn new(start_ms: f64, duration_ms: f64) -> Self {
        Self {
            start_ms,
            duration_ms: duration_ms.max(1.0),
            previous_eased: 0.0,
        }
    }

    /// The raw progress at `now_ms`, `0..=1`.
    pub fn progress(&self, now_ms: f64) -> f32 {
        (((now_ms - self.start_ms) / self.duration_ms) as f32).clamp(0.0, 1.0)
    }

    /// Whether the raw progress reached 1.
    pub fn finished(&self, now_ms: f64) -> bool {
        self.progress(now_ms) >= 1.0
    }

    /// The fraction of the current remaining distance to move this frame.
    pub fn frame_fraction(&mut self, now_ms: f64) -> f32 {
        let eased = navigation_ease(self.progress(now_ms));
        let previous = self.previous_eased;
        self.previous_eased = eased;
        let denominator = 1.0 - previous;
        if denominator <= 1e-6 {
            1.0
        } else {
            ((eased - previous) / denominator).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a2_break_and_repin_thresholds() {
        // Break at D > previous + 1 and D > 2.
        let mut policy = FollowPolicy::new();
        assert_eq!(policy.on_user_scroll(1.5), PinChange::Unchanged);
        assert!(policy.is_pinned());
        assert_eq!(policy.on_user_scroll(2.0), PinChange::Unchanged);
        // 2 -> 3 is not more than previous + 1 (it equals): stays pinned.
        assert_eq!(policy.on_user_scroll(3.0), PinChange::Unchanged);
        assert!(policy.is_pinned());
        assert_eq!(policy.on_user_scroll(4.5), PinChange::Broke);
        assert!(!policy.is_pinned());

        // D <= 2 re-pins even when not moving toward the bottom (previous was 0 or smaller).
        let mut policy = FollowPolicy::new();
        policy.set_pinned(false);
        policy.record_distance(0.5);
        assert_eq!(policy.on_user_scroll(2.0), PinChange::Repinned);

        // D = 70 moving toward re-pins.
        let mut policy = FollowPolicy::new();
        policy.set_pinned(false);
        policy.record_distance(100.0);
        assert_eq!(policy.on_user_scroll(70.0), PinChange::Repinned);
        // D = 71 does not.
        let mut policy = FollowPolicy::new();
        policy.set_pinned(false);
        policy.record_distance(100.0);
        assert_eq!(policy.on_user_scroll(71.0), PinChange::Unchanged);
        assert!(!policy.is_pinned());
        // D = 60 moving away does not.
        let mut policy = FollowPolicy::new();
        policy.set_pinned(false);
        policy.record_distance(50.0);
        assert_eq!(policy.on_user_scroll(60.0), PinChange::Unchanged);
        assert!(!policy.is_pinned());
    }

    #[test]
    fn a_wheel_up_notch_near_the_bottom_stays_detached() {
        let mut policy = FollowPolicy::new();
        assert_eq!(policy.on_user_scroll(40.0), PinChange::Broke);
        // Notch up 20 px further inside the band, then a smaller one: stay detached.
        assert_eq!(policy.on_user_scroll(60.0), PinChange::Unchanged);
        assert!(!policy.is_pinned());
        assert_eq!(policy.on_user_scroll(55.0), PinChange::Repinned);
    }

    #[test]
    fn a5_jump_button_hysteresis() {
        let mut policy = FollowPolicy::new();
        policy.set_pinned(false);
        assert!(!policy.update_jump_button(320.0));
        assert!(policy.update_jump_button(320.5));
        // Persists while the distance falls, until within 2 px.
        assert!(policy.update_jump_button(200.0));
        assert!(policy.update_jump_button(2.5));
        assert!(!policy.update_jump_button(2.0));
        // Not offered again until beyond 320.
        assert!(!policy.update_jump_button(300.0));
        // A pinned list never offers it.
        policy.set_pinned(true);
        assert!(!policy.update_jump_button(500.0));
    }

    #[test]
    fn a5_long_jumps_teleport_the_excess_over_two_and_a_half_viewports() {
        assert_eq!(teleport_excess(5000.0, 500.0), 5000.0 - 1250.0);
        assert_eq!(teleport_excess(1250.0, 500.0), 0.0);
        assert_eq!(teleport_excess(1000.0, 500.0), 0.0);
        assert_eq!(teleport_excess(1000.0, 0.0), 0.0);
        // Ten viewports away.
        assert_eq!(teleport_excess(10.0 * 400.0, 400.0), 4000.0 - 1000.0);
    }

    #[test]
    fn a3_the_stepper_never_overshoots_is_monotone_and_lands_exactly() {
        let mut stepper = Stepper::new();
        let target = 1000.0;
        let mut position = 0.0;
        let mut previous = position;
        let mut landed = false;
        for _ in 0..600 {
            position = stepper.step(position, target, 1.0);
            assert!(position <= target, "overshoot {position}");
            assert!(position >= previous, "not monotone {previous} {position}");
            previous = position;
            if position == target {
                landed = true;
                break;
            }
        }
        assert!(landed, "the stepper lands exactly on the target");
        assert!(stepper.is_idle(position, target));
    }

    #[test]
    fn a3_the_lead_behind_a_growing_target_stays_bounded() {
        let mut stepper = Stepper::new();
        let mut target = 100.0;
        let mut position = 100.0;
        for frame in 0..400 {
            target += 3.0;
            position = stepper.step(position, target, 1.0);
            if frame > 30 {
                let lead = target - position;
                assert!(lead <= 32.0 + 3.0, "lead {lead} at frame {frame}");
            }
        }
        assert!(stepper.growth() > 2.0 && stepper.growth() < 3.5);
    }

    #[test]
    fn a3_a_target_shrink_zeroes_the_growth_estimate_and_it_goes_idle_when_growth_stops() {
        let mut stepper = Stepper::new();
        let mut target = 0.0;
        let mut position = 0.0;
        for _ in 0..60 {
            target += 3.0;
            position = stepper.step(position, target, 1.0);
        }
        assert!(stepper.growth() > 1.0);
        target -= 5.0;
        position = stepper.step(position.min(target), target, 1.0);
        assert_eq!(stepper.growth(), 0.0);
        // The target stops growing: the stepper lands and reports idle.
        for _ in 0..200 {
            position = stepper.step(position, target, 1.0);
        }
        assert!(stepper.is_idle(position, target));
        assert_eq!(position, target);
    }

    #[test]
    fn a_timed_tick_caps_catch_up_and_resets_cold_after_the_settle_grace() {
        let mut stepper = Stepper::new();
        // A first tick counts one frame; a big hitch counts at most eight.
        let a = stepper.tick(0.0, 0.0, 10_000.0);
        let mut other = Stepper::new();
        let b = other.step(0.0, 10_000.0, 1.0);
        assert_eq!(a, b);
        let mut hitch = stepper.clone();
        let c = hitch.tick(10_000.0, a, 10_000.0);
        let mut eight = stepper.clone();
        let d = eight.step(a, 10_000.0, 8.0);
        assert_eq!(c, d);
        // After landing, the state survives 500 ms and then resets cold.
        let mut s = Stepper::new();
        let mut position = 0.0;
        let mut now = 0.0;
        while !s.is_idle(position, 100.0) {
            now += FRAME_MS;
            position = s.tick(now, position, 100.0);
        }
        let landed_at = now;
        assert!(s.velocity() > 0.0, "spring state is retained after landing");
        let mut warm = s.clone();
        warm.tick(landed_at + 400.0, 100.0, 100.0);
        assert!(warm.velocity() > 0.0, "still warm inside the 500 ms grace");
        let mut cold = s.clone();
        cold.tick(landed_at + 1200.0, 100.0, 100.0);
        assert_eq!(cold.velocity(), 0.0, "reset cold after the grace");
        assert_eq!(cold.growth(), 0.0);
    }

    #[test]
    fn reservations_clamp_and_ignore_jitter() {
        assert_eq!(reserved_height(120.0, 100.0), 100.0);
        assert_eq!(reserved_height(-5.0, 100.0), 0.0);
        assert_eq!(reserved_height(80.0, 100.0), 80.0);
        assert_eq!(list_viewport_height(500.0, 120.0), 380.0);
        assert_eq!(list_viewport_height(100.0, 120.0), 0.0);
        assert!(!reservation_changed(100.0, 100.4));
        assert!(reservation_changed(100.0, 100.6));
        assert!(reservation_changed(0.0, 120.0));
    }

    #[test]
    fn autoscroll_steps_ramp_quadratically_inside_the_edge_zones() {
        // 300 px viewport at 100..400: zone 36.
        assert_eq!(autoscroll_step(250.0, 100.0, 400.0), 0.0);
        assert_eq!(autoscroll_step(136.0, 100.0, 400.0), 0.0);
        assert_eq!(autoscroll_step(100.0, 100.0, 400.0), -24.0);
        assert_eq!(autoscroll_step(40.0, 100.0, 400.0), -24.0);
        assert!((autoscroll_step(118.0, 100.0, 400.0) - -6.0).abs() < 1e-4);
        assert_eq!(autoscroll_step(400.0, 100.0, 400.0), 24.0);
        assert!((autoscroll_step(382.0, 100.0, 400.0) - 6.0).abs() < 1e-4);
        // The zone is a third of a short viewport.
        assert_eq!(autoscroll_step(10.0, 0.0, 30.0), 0.0 - 24.0 * 0.0);
        assert_eq!(autoscroll_step(0.0, 0.0, 30.0), -24.0);
        assert_eq!(autoscroll_step(5.0, 5.0, 5.0), 0.0);
    }

    #[test]
    fn the_navigation_timeline_telescopes_to_the_eased_position() {
        let mut timeline = NavigationTimeline::new(0.0, 500.0);
        let mut remaining = 1000.0f32;
        let total = remaining;
        let mut now = 0.0;
        while now < 500.0 {
            now += 16.0;
            let fraction = timeline.frame_fraction(now);
            remaining -= remaining * fraction;
            let expected = total * (1.0 - navigation_ease(timeline.progress(now)));
            assert!(
                (remaining - expected).abs() < 0.5,
                "{remaining} vs {expected}"
            );
        }
        assert!(remaining.abs() < 0.5);
        assert!((navigation_ease(0.5) - 0.5).abs() < 1e-3);
        assert!(navigation_ease(0.25) < 0.25 && navigation_ease(0.75) > 0.75);
    }
}
