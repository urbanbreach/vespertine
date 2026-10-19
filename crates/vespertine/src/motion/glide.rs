//! The retargetable critically damped geometry glide (ANM-002 R9) and the route/reflow glide
//! pair (R10).

use std::time::Duration;

/// The glide is active while the distance to its target exceeds this many pixels.
pub const GLIDE_DISTANCE_THRESHOLD: f32 = 0.0005;
/// The glide is active while its absolute velocity exceeds this many pixels per second.
pub const GLIDE_VELOCITY_THRESHOLD: f32 = 0.005;

/// A critically damped retargetable motion of one scalar: position `value` and `velocity`.
///
/// For a glide of duration `D` seconds the angular rate is `w = 12 / D`. Advancing by `dt` toward
/// a fixed target `T` uses the closed form `d = x - T`, `c = v + w d`, `decay = exp(-w dt)`,
/// `x' = T + (d + c dt) decay`, `v' = (v - w c dt) decay`, which is frame-rate independent and
/// never oscillates. Changing the target between advances preserves position and velocity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Glide {
    /// The current value.
    pub value: f32,
    /// The current velocity in units per second.
    pub velocity: f32,
}

impl Glide {
    /// A glide at rest at `value`.
    pub const fn at(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
        }
    }

    /// The angular rate for a glide of the given duration: twelve time constants over `D`.
    /// A non-positive duration yields infinity (an immediate snap).
    pub fn angular_rate(duration: Duration) -> f64 {
        let seconds = duration.as_secs_f64();
        if seconds > 0.0 {
            12.0 / seconds
        } else {
            f64::INFINITY
        }
    }

    /// The closed-form state after `dt` seconds with a fixed target, without any snapping.
    pub fn step(self, target: f32, duration: Duration, dt: f32) -> Glide {
        let w = Self::angular_rate(duration);
        if !w.is_finite() {
            return Glide::at(target);
        }
        let dt = f64::from(dt.max(0.0));
        let d = f64::from(self.value) - f64::from(target);
        let c = f64::from(self.velocity) + w * d;
        let decay = (-w * dt).exp();
        Glide {
            value: (f64::from(target) + (d + c * dt) * decay) as f32,
            velocity: ((f64::from(self.velocity) - w * c * dt) * decay) as f32,
        }
    }

    /// Whether the glide still needs frames: its distance to `target` exceeds the distance
    /// threshold or its velocity exceeds the velocity threshold.
    pub fn is_active(&self, target: f32) -> bool {
        (self.value - target).abs() > GLIDE_DISTANCE_THRESHOLD
            || self.velocity.abs() > GLIDE_VELOCITY_THRESHOLD
    }

    /// Advances by `dt` seconds toward `target`. As soon as the advance leaves the glide
    /// inactive it snaps exactly to the target with zero velocity. Returns whether the glide is
    /// still active (and so needs another frame).
    pub fn advance(&mut self, target: f32, duration: Duration, dt: f32) -> bool {
        *self = self.step(target, duration, dt);
        if self.is_active(target) {
            true
        } else {
            *self = Glide::at(target);
            false
        }
    }

    /// Jumps exactly to `target` with zero velocity (reduced motion).
    pub fn snap(&mut self, target: f32) {
        *self = Glide::at(target);
    }

    /// Adds `delta` to the value, keeping the velocity: used to absorb a layout position change
    /// into a glide that decays to a fixed target.
    pub fn displace(&mut self, delta: f32) {
        self.value += delta;
    }
}

/// A two-axis displacement made of a route glide and an independent reflow glide, both decaying
/// to zero (ANM-002 R10).
///
/// A route change adds the natural-position delta to the route glide; a layout change with the
/// same route adds its delta to the reflow glide. Neither ever restarts, resets or retargets the
/// other. The displayed displacement is the sum of both.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TravelGlide {
    /// Route glide per axis (x, y).
    pub route: [Glide; 2],
    /// Reflow glide per axis (x, y).
    pub reflow: [Glide; 2],
}

impl TravelGlide {
    /// Absorbs a layout position change `delta` (old natural position minus new natural
    /// position, per axis) into the route glide when `route_changed`, otherwise into the reflow
    /// glide.
    pub fn displace(&mut self, delta: [f32; 2], route_changed: bool) {
        let glides = if route_changed {
            &mut self.route
        } else {
            &mut self.reflow
        };
        for (glide, delta) in glides.iter_mut().zip(delta) {
            glide.displace(delta);
        }
    }

    /// Advances both glides toward zero; each uses its own duration. Under reduced motion both
    /// snap to zero. Returns whether either glide needs another frame.
    pub fn advance(
        &mut self,
        route_duration: Duration,
        reflow_duration: Duration,
        dt: f32,
        reduced_motion: bool,
    ) -> bool {
        let mut active = false;
        for (glides, duration) in [
            (&mut self.route, route_duration),
            (&mut self.reflow, reflow_duration),
        ] {
            for glide in glides.iter_mut() {
                if reduced_motion {
                    glide.snap(0.0);
                } else {
                    active |= glide.advance(0.0, duration, dt);
                }
            }
        }
        active
    }

    /// The displayed displacement (route plus reflow) per axis.
    pub fn displacement(&self) -> [f32; 2] {
        [
            self.route[0].value + self.reflow[0].value,
            self.route[1].value + self.reflow[1].value,
        ]
    }
}
