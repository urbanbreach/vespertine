//! Motion seam for pane composition.
//!
//! ANM-002 owns the shared declarative motion API. Until it lands, pane slots
//! and moving surfaces use these small types, which implement exactly the
//! formulas ANM-002 fixes (cubic-bezier easing with 20 binary subdivisions,
//! retargeting from the currently displayed value, and the critically damped
//! geometry glide). They are the single replacement point: every consumer in
//! `panes` goes through [`RetargetableTransition`] and [`Glide`].

use std::time::{Duration, Instant};

use crate::{SpringConfig, SpringState};

/// Duration of the size transition used for pane slots (ANM-002 preset).
pub const SIZE_TRANSITION_DURATION: Duration = Duration::from_millis(200);

/// A cubic-bezier easing curve with end points (0, 0) and (1, 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    /// First control point x.
    pub x1: f32,
    /// First control point y.
    pub y1: f32,
    /// Second control point x.
    pub x2: f32,
    /// Second control point y.
    pub y2: f32,
}

impl CubicBezier {
    /// The size-transition curve of ANM-002: cubic-bezier(0, 0, 0.58, 1).
    pub const SIZE: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);

    /// Creates a curve from its control points.
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    fn component(a: f64, b: f64, t: f64) -> f64 {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    }

    /// Eases `t` (clamped to 0..=1). Solves x(s) = t by twenty binary
    /// subdivisions and returns y(s) clamped to 0..=1.
    pub fn ease(&self, t: f32) -> f32 {
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        let t = t as f64;
        let (mut low, mut high) = (0.0f64, 1.0f64);
        let mut mid = 0.5;
        for _ in 0..20 {
            mid = (low + high) * 0.5;
            if Self::component(self.x1 as f64, self.x2 as f64, mid) < t {
                low = mid;
            } else {
                high = mid;
            }
        }
        Self::component(self.y1 as f64, self.y2 as f64, mid).clamp(0.0, 1.0) as f32
    }
}

/// A scalar that eases toward a target and can be retargeted mid-flight from
/// its currently displayed value without a jump.
#[derive(Clone, Copy, Debug)]
pub struct RetargetableTransition {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    curve: CubicBezier,
}

impl RetargetableTransition {
    /// Creates a transition at rest at `value`.
    pub fn at_rest(value: f32, now: Instant, duration: Duration, curve: CubicBezier) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
            duration,
            curve,
        }
    }

    /// The value the transition is heading to.
    pub fn target(&self) -> f32 {
        self.to
    }

    /// The displayed value at `now`.
    pub fn value(&self, now: Instant) -> f32 {
        if self.from == self.to {
            return self.to;
        }
        let elapsed = now.saturating_duration_since(self.start);
        if self.duration.is_zero() || elapsed >= self.duration {
            return self.to;
        }
        let progress = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        self.from + (self.to - self.from) * self.curve.ease(progress)
    }

    /// Whether the displayed value is still changing at `now`.
    pub fn is_active(&self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.start) < self.duration
    }

    /// Retargets from the value displayed at `now`. Retargeting to the current
    /// target is a no-op and does not restart time.
    pub fn retarget(&mut self, now: Instant, to: f32) {
        if to == self.to {
            return;
        }
        self.from = self.value(now);
        self.to = to;
        self.start = now;
    }

    /// Jumps to `value` with no animation.
    pub fn snap(&mut self, now: Instant, value: f32) {
        self.from = value;
        self.to = value;
        self.start = now;
    }

    /// Changes the duration and curve used by later retargets.
    pub fn set_timing(&mut self, duration: Duration, curve: CubicBezier) {
        if self.from == self.to {
            self.duration = duration;
            self.curve = curve;
        }
    }
}

/// Distance below which a glide counts as arrived (ANM-002 R9).
const GLIDE_DISTANCE: f32 = 0.0005;
/// Speed below which a glide counts as arrived (ANM-002 R9).
const GLIDE_VELOCITY: f32 = 0.005;

/// A critically damped, retargetable scalar motion (ANM-002 R9). With
/// duration `D` the angular rate is `12 / D`; this is the upstream spring with
/// stiffness `w^2`, damping `2w` and mass 1.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    state: SpringState,
    target: f32,
    updated: Instant,
    duration: Duration,
}

impl Glide {
    /// Creates a glide at rest at `value`.
    pub fn at_rest(value: f32, now: Instant, duration: Duration) -> Self {
        Self {
            state: SpringState {
                position: value,
                velocity: 0.0,
            },
            target: value,
            updated: now,
            duration,
        }
    }

    fn config(&self) -> SpringConfig {
        let w = 12.0 / self.duration.as_secs_f32().max(1e-4);
        SpringConfig::new(w * w, 2.0 * w, 1.0)
    }

    /// Whether the glide is still moving.
    pub fn is_active(&self) -> bool {
        (self.state.position - self.target).abs() > GLIDE_DISTANCE
            || self.state.velocity.abs() > GLIDE_VELOCITY
    }

    /// The target value.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// Advances to `now` and returns the displayed value. Snaps exactly to the
    /// target with zero velocity as soon as the glide becomes inactive.
    pub fn advance(&mut self, now: Instant) -> f32 {
        let dt = now.saturating_duration_since(self.updated).as_secs_f32();
        self.updated = self.updated.max(now);
        if dt > 0.0 && self.is_active() {
            self.state = self.config().step(self.state, self.target, dt);
            if !self.is_active() {
                self.snap_to_target();
            }
        }
        self.state.position
    }

    /// The currently displayed value without advancing time.
    pub fn current(&self) -> f32 {
        self.state.position
    }

    /// Advances to `now`, then retargets, preserving position and velocity.
    pub fn retarget(&mut self, now: Instant, target: f32, duration: Duration) {
        self.advance(now);
        self.target = target;
        self.duration = duration;
        if !self.is_active() {
            self.snap_to_target();
        }
    }

    /// Advances to `now` and adds `delta` to the displayed value, keeping the
    /// velocity. Used to absorb a layout-position change.
    pub fn displace(&mut self, now: Instant, delta: f32) {
        self.advance(now);
        self.state.position += delta;
    }

    /// Jumps to the target with zero velocity.
    pub fn snap_to_target(&mut self) {
        self.state = SpringState {
            position: self.target,
            velocity: 0.0,
        };
    }

    /// The current velocity.
    pub fn velocity(&self) -> f32 {
        self.state.velocity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_curve_endpoints_and_monotonic() {
        let curve = CubicBezier::SIZE;
        assert_eq!(curve.ease(0.0), 0.0);
        assert_eq!(curve.ease(1.0), 1.0);
        let mut previous = 0.0;
        for i in 1..=100 {
            let value = curve.ease(i as f32 / 100.0);
            assert!(value >= previous);
            previous = value;
        }
    }

    #[test]
    fn transition_retargets_from_displayed_value() {
        let t0 = Instant::now();
        let mut transition =
            RetargetableTransition::at_rest(0.0, t0, Duration::from_millis(200), CubicBezier::SIZE);
        transition.retarget(t0, 100.0);
        let mid = t0 + Duration::from_millis(100);
        let displayed = transition.value(mid);
        assert!(displayed > 0.0 && displayed < 100.0);
        transition.retarget(mid, 0.0);
        assert_eq!(transition.value(mid), displayed);
        assert_eq!(transition.value(mid + Duration::from_millis(200)), 0.0);
    }

    #[test]
    fn glide_settles_without_overshoot() {
        let t0 = Instant::now();
        let mut glide = Glide::at_rest(0.0, t0, Duration::from_millis(420));
        glide.retarget(t0, 2000.0, Duration::from_millis(420));
        let mut now = t0;
        for _ in 0..60 {
            now += Duration::from_millis(16);
            let value = glide.advance(now);
            assert!(value <= 2000.0);
        }
        assert!((glide.advance(now + Duration::from_millis(100)) - 2000.0).abs() < 0.5);
    }
}
