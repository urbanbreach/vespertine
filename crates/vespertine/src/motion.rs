//! Native declarative motion (ANM-002) and presence management (ANM-003).
//!
//! - [`MotionTrack`] / [`MotionDescription`]: declarative property tracks with delay, duration,
//!   named and cubic-bezier easing, retargeting from the visible value and exactly-once
//!   completion reports carrying a generation.
//! - [`Glide`] / [`TravelGlide`] / [`Travel`]: the critically damped retargetable geometry glide
//!   with independent route and reflow components.
//! - [`PaintScope`]: paint-local opacity, displacement and inertness without relayout.
//! - [`PresenceLifecycle`] / [`PresenceManager`]: exit lifecycle (open, closing, unmounted) and
//!   the presence tracking logic retained-tree adapters call.
//! - [`label`]: the changed-glyph rolling label.
//! - [`presets`]: the timing vocabulary and the optional global timeline scale.

mod easing;
mod elements;
mod glide;
pub mod label;
mod presence;
mod track;

use std::{
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};

use crate::App;

pub use easing::{CubicBezier, Easing, MotionError, NamedEasing};
pub use elements::{MotionCompleteHandler, MotionElement, MotionExt, PaintScope, Travel};
pub use glide::{GLIDE_DISTANCE_THRESHOLD, GLIDE_VELOCITY_THRESHOLD, Glide, TravelGlide};
pub use label::RollingLabel;
pub use presence::{
    ParticipantId, PresenceChild, PresenceContext, PresenceFlavor, PresenceItem, PresenceLifecycle,
    PresenceManager, PresenceOrder, PresenceOutcome, PresenceState, RemovalMode, SafeToRemove,
    spawn_fallback_timer, use_is_present, use_presence,
};
pub use track::{
    DEFAULT_DURATION_SECS, MotionCompletion, MotionDescription, MotionInitial, MotionProperty,
    MotionProps, MotionSample, MotionStyle, MotionTrack, MotionTransition, raw_progress,
};

/// Elapsed time on the clock that drives motion tracks, glides and label rolls.
///
/// This is the single place elements read time; it follows the executor's clock, so it is
/// deterministic under the test platform.
pub(crate) fn now(cx: &App) -> Duration {
    cx.background_executor()
        .now()
        .saturating_duration_since(cx.synced_animation_epoch)
}

static TIMELINE_SCALE_BITS: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0f32

/// Sets the global timeline scale that multiplies preset durations, clamped to `0.01..=100`.
/// Non-finite values are ignored. This is for deterministic captures and measurement, not a
/// styling feature.
pub fn set_timeline_scale(scale: f32) {
    if scale.is_finite() {
        TIMELINE_SCALE_BITS.store(scale.clamp(0.01, 100.0).to_bits(), Ordering::Relaxed);
    }
}

/// The global timeline scale (default 1).
pub fn timeline_scale() -> f32 {
    f32::from_bits(TIMELINE_SCALE_BITS.load(Ordering::Relaxed))
}

/// The timing presets of ANM-002.
pub mod presets {
    use std::time::Duration;

    use super::{CubicBezier, Easing, MotionTransition, timeline_scale};

    /// A duration and curve preset. [`Preset::duration`] applies the global timeline scale.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Preset {
        millis: u32,
        /// The easing curve of the preset. Glide presets are critically damped and ignore it.
        pub easing: Easing,
    }

    impl Preset {
        const fn new(millis: u32, easing: Easing) -> Self {
            Self { millis, easing }
        }

        /// The unscaled duration in milliseconds.
        pub const fn millis(&self) -> u32 {
            self.millis
        }

        /// The duration multiplied by the global timeline scale.
        pub fn duration(&self) -> Duration {
            Duration::from_secs_f64(f64::from(self.millis) / 1000.0 * f64::from(timeline_scale()))
        }

        /// The preset as a declarative transition (duration scaled by the timeline scale).
        pub fn transition(&self) -> MotionTransition {
            MotionTransition::new()
                .duration(self.duration().as_secs_f64())
                .easing(self.easing.clone())
        }
    }

    /// Entrance: 500 ms with `cubic-bezier(0.16, 1, 0.3, 1)`, together with a 4 px rise.
    pub const ENTRANCE: Preset = Preset::new(500, Easing::cubic_bezier(0.16, 1.0, 0.3, 1.0));
    /// The rise that accompanies [`ENTRANCE`], in pixels.
    pub const ENTRANCE_RISE_PX: f32 = 4.0;
    /// Quick fade: 150 ms with `ease`.
    pub const QUICK_FADE: Preset = Preset::new(150, Easing::EASE);
    /// Menu entry: 140 ms with `ease`.
    pub const MENU_ENTRY: Preset = Preset::new(140, Easing::EASE);
    /// Menu exit: 100 ms with `ease`.
    pub const MENU_EXIT: Preset = Preset::new(100, Easing::EASE);
    /// Dialog entry: 180 ms with `ease`.
    pub const DIALOG_ENTRY: Preset = Preset::new(180, Easing::EASE);
    /// Size transitions: 200 ms with `cubic-bezier(0, 0, 0.58, 1)`.
    pub const SIZE_TRANSITION: Preset = Preset::new(200, Easing::cubic_bezier(0.0, 0.0, 0.58, 1.0));
    /// The size curve as a bare Bezier.
    pub const SIZE_CURVE: CubicBezier = CubicBezier::new(0.0, 0.0, 0.58, 1.0);
    /// Retained-input docking glide: 420 ms (critically damped; the curve is unused).
    pub const DOCKING_GLIDE: Preset = Preset::new(420, Easing::LINEAR);
    /// Retained-input return glide: 470 ms (critically damped; the curve is unused).
    pub const RETURN_GLIDE: Preset = Preset::new(470, Easing::LINEAR);
    /// Background crossfade: 180 ms with `cubic-bezier(1/3, 1, 2/3, 1)`.
    pub const BACKGROUND_CROSSFADE: Preset =
        Preset::new(180, Easing::cubic_bezier(1.0 / 3.0, 1.0, 2.0 / 3.0, 1.0));
    /// Label roll: 550 ms with the glyph curve [`LABEL_GLYPH_CURVE`].
    pub const LABEL_ROLL: Preset = Preset::new(550, Easing::cubic_bezier(0.22, 1.0, 0.36, 1.0));
    /// The label-roll glyph curve `cubic-bezier(0.22, 1, 0.36, 1)` (no overshoot, monotonic).
    pub const LABEL_GLYPH_CURVE: CubicBezier = CubicBezier::new(0.22, 1.0, 0.36, 1.0);
    /// Label-roll travel, in ems.
    pub const LABEL_TRAVEL_EM: f32 = 0.35;
    /// Label-roll resting scale of old and incoming glyphs.
    pub const LABEL_REST_SCALE: f32 = 0.6;
    /// Label-roll tilt in degrees at full motion.
    pub const LABEL_TILT_DEGREES: f32 = 2.0;
    /// Label-roll blur in ems at full motion.
    pub const LABEL_BLUR_EM: f32 = 0.1;
    /// Label-roll stagger as a fraction of the duration.
    pub const LABEL_STAGGER: f32 = 0.3;
    /// The whole label roll lasts the roll duration times this factor.
    pub const LABEL_TOTAL_FACTOR: f64 = 1.3;
}

#[cfg(test)]
mod tests;
