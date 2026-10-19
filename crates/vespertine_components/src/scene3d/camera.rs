//! The orbit camera and its pointer interaction (CMP-025 R3, R4, R5; shared with CMP-026 R7).
//!
//! [`OrbitController`] is pure state: it never reads a clock or a window, so the orbit, zoom and
//! turntable rules are tested without a frame. The surfaces feed it pointer positions and the
//! current instant.

use std::time::Instant;
use vespertine::{Pixels, Point, TouchPhase};

/// Default yaw in radians.
pub const DEFAULT_YAW: f32 = 0.6;
/// Default pitch in radians.
pub const DEFAULT_PITCH: f32 = 0.45;
/// Default zoom.
pub const DEFAULT_ZOOM: f32 = 1.0;
/// Pitch limit in radians (both signs).
pub const PITCH_LIMIT: f32 = 1.5;
/// Smallest zoom.
pub const ZOOM_MIN: f32 = 0.2;
/// Largest zoom.
pub const ZOOM_MAX: f32 = 5.0;
/// Turntable limit in radians per second (both signs).
pub const SPIN_LIMIT: f32 = 3.0;
/// Radians per logical pixel of orbit drag.
pub const ORBIT_SENSITIVITY: f32 = 0.01;
/// Default zoom factor per wheel notch.
pub const DEFAULT_ZOOM_STEP: f32 = 1.1;
/// One wheel notch in pixel-delta units.
pub const NOTCH_UNITS: f32 = 120.0;
/// Smallest cumulative pinch factor.
pub const PINCH_FLOOR: f32 = 0.001;

/// An orbit camera: yaw, pitch and zoom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Rotation about the vertical axis in radians.
    pub yaw: f32,
    /// Elevation in radians, within [-1.5, 1.5].
    pub pitch: f32,
    /// Zoom factor, within [0.2, 5.0].
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: DEFAULT_YAW,
            pitch: DEFAULT_PITCH,
            zoom: DEFAULT_ZOOM,
        }
    }
}

impl Camera {
    /// A camera with pitch and zoom clamped to their limits.
    pub fn new(yaw: f32, pitch: f32, zoom: f32) -> Self {
        Self { yaw, pitch, zoom }.clamped()
    }

    /// This camera with pitch and zoom clamped (non-finite values fall back to the defaults).
    pub fn clamped(self) -> Self {
        let finite = |v: f32, default: f32| if v.is_finite() { v } else { default };
        Self {
            yaw: finite(self.yaw, DEFAULT_YAW),
            pitch: finite(self.pitch, DEFAULT_PITCH).clamp(-PITCH_LIMIT, PITCH_LIMIT),
            zoom: finite(self.zoom, DEFAULT_ZOOM).clamp(ZOOM_MIN, ZOOM_MAX),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Press {
    position: Point<Pixels>,
    yaw: f32,
    pitch: f32,
}

/// Camera state, orbit drag, wheel and pinch zoom and the turntable of one surface.
#[derive(Clone, Debug)]
pub struct OrbitController {
    camera: Camera,
    orbit: bool,
    spin: f32,
    spin_epoch: Instant,
    zoom_step: f32,
    press: Option<Press>,
    pinch: Option<(f32, f32)>,
}

impl OrbitController {
    /// A controller at `camera` with orbit enabled and no turntable; the spin epoch is `now`.
    pub fn new(camera: Camera, now: Instant) -> Self {
        Self {
            camera: camera.clamped(),
            orbit: true,
            spin: 0.0,
            spin_epoch: now,
            zoom_step: DEFAULT_ZOOM_STEP,
            press: None,
            pinch: None,
        }
    }

    /// Enables or disables orbit dragging.
    pub fn set_orbit(&mut self, orbit: bool) {
        self.orbit = orbit;
        if !orbit {
            self.press = None;
        }
    }

    /// Whether orbit dragging is enabled.
    pub fn orbit(&self) -> bool {
        self.orbit
    }

    /// Sets the turntable in radians per second, clamped to [-3, 3].
    pub fn set_spin(&mut self, spin: f32, now: Instant) {
        let spin = if spin.is_finite() {
            spin.clamp(-SPIN_LIMIT, SPIN_LIMIT)
        } else {
            0.0
        };
        if spin != self.spin {
            // Fold what the old turntable accumulated so changing it does not jump.
            self.camera.yaw +=
                self.spin * now.saturating_duration_since(self.spin_epoch).as_secs_f32();
            self.spin_epoch = now;
            self.spin = spin;
        }
    }

    /// The turntable in radians per second.
    pub fn spin(&self) -> f32 {
        self.spin
    }

    /// Sets the zoom factor per wheel notch.
    pub fn set_zoom_step(&mut self, step: f32) {
        if step.is_finite() && step > 0.0 {
            self.zoom_step = step;
        }
    }

    /// The zoom factor per wheel notch.
    pub fn zoom_step(&self) -> f32 {
        self.zoom_step
    }

    /// Replaces the stored camera (clamped).
    pub fn set_camera(&mut self, camera: Camera, now: Instant) {
        self.camera = camera.clamped();
        self.spin_epoch = now;
    }

    /// The stored camera: the yaw does not include the turntable.
    pub fn stored(&self) -> Camera {
        self.camera
    }

    /// Whether a drag is active.
    pub fn dragging(&self) -> bool {
        self.press.is_some()
    }

    /// Whether the turntable is running at `now`: a non-zero spin, no drag, motion permitted.
    pub fn turntable_running(&self, reduce_motion: bool) -> bool {
        self.spin != 0.0 && !reduce_motion && self.press.is_none()
    }

    /// The camera to display at `now`: the stored camera plus `spin x elapsed seconds` of yaw
    /// while no drag is active and motion is permitted.
    pub fn displayed(&self, now: Instant, reduce_motion: bool) -> Camera {
        let mut camera = self.camera;
        if self.turntable_running(reduce_motion) {
            camera.yaw += self.spin * now.saturating_duration_since(self.spin_epoch).as_secs_f32();
        }
        camera
    }

    /// A primary-button press at `position` (logical pixels): folds the turntable yaw into the
    /// stored yaw and records the press. Returns whether the press started a drag (orbit
    /// enabled); the caller stops propagation and captures the pointer then.
    pub fn press(&mut self, position: Point<Pixels>, now: Instant, reduce_motion: bool) -> bool {
        if !self.orbit {
            return false;
        }
        if !reduce_motion {
            self.camera.yaw +=
                self.spin * now.saturating_duration_since(self.spin_epoch).as_secs_f32();
        }
        self.spin_epoch = now;
        self.press = Some(Press {
            position,
            yaw: self.camera.yaw,
            pitch: self.camera.pitch,
        });
        true
    }

    /// A pointer move during a drag: `yaw = yaw_at_press - dx x 0.01`,
    /// `pitch = clamp(pitch_at_press + dy x 0.01, -1.5, 1.5)`. Returns whether the camera
    /// changed.
    pub fn drag_to(&mut self, position: Point<Pixels>) -> bool {
        let Some(press) = self.press else {
            return false;
        };
        let dx = f32::from(position.x) - f32::from(press.position.x);
        let dy = f32::from(position.y) - f32::from(press.position.y);
        if !dx.is_finite() || !dy.is_finite() {
            return false;
        }
        let before = self.camera;
        self.camera.yaw = press.yaw - dx * ORBIT_SENSITIVITY;
        self.camera.pitch = (press.pitch + dy * ORBIT_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.camera != before
    }

    /// Ends the drag (inside or outside the surface) and restarts the spin epoch at `now`.
    /// Returns whether a drag was active.
    pub fn release(&mut self, now: Instant) -> bool {
        let was = self.press.take().is_some();
        if was {
            self.spin_epoch = now;
        }
        was
    }

    /// Wheel zoom by `notches` (positive scrolls up and zooms in): the zoom is multiplied by
    /// `step^notches` and clamped to [0.2, 5.0].
    pub fn wheel(&mut self, notches: f32) {
        if notches.is_finite() {
            self.camera.zoom =
                (self.camera.zoom * self.zoom_step.powf(notches)).clamp(ZOOM_MIN, ZOOM_MAX);
        }
    }

    /// A pinch event: the zoom becomes the zoom at the start of the pinch times the cumulative
    /// scale factor (the product of `1 + delta`), clamped to [0.2, 5.0].
    pub fn pinch(&mut self, phase: TouchPhase, delta: f32) {
        match phase {
            TouchPhase::Started => self.pinch = Some((self.camera.zoom, 1.0)),
            TouchPhase::Moved => {
                if !delta.is_finite() {
                    return;
                }
                let (start, factor) = self.pinch.unwrap_or((self.camera.zoom, 1.0));
                let factor = (factor * (1.0 + delta)).max(PINCH_FLOOR);
                self.pinch = Some((start, factor));
                self.camera.zoom = (start * factor).clamp(ZOOM_MIN, ZOOM_MAX);
            }
            TouchPhase::Ended | TouchPhase::Cancelled => self.pinch = None,
        }
    }
}

/// Notches of a wheel delta: line deltas count one notch per line, pixel deltas one per 120
/// units.
pub fn wheel_notches(delta: vespertine::ScrollDelta) -> f32 {
    match delta {
        vespertine::ScrollDelta::Lines(lines) => lines.y,
        vespertine::ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / NOTCH_UNITS,
    }
}
