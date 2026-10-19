//! The retargetable scalar of the switch thumb and the discrete slider thumb (CMP-001 R13, R21).
//!
//! A [`ScalarMotion`] is a position between 0 and 1 that travels from its current value to a
//! target in a fixed duration along an ease-out-cubic curve:
//! `position(t) = from + (target - from) * (1 - (1 - t)^3)`, `t = elapsed / duration` clamped
//! to 1. Changing the target during travel restarts from the *current* position, so the thumb
//! never jumps. Under reduced motion the position snaps to the target. Frames are needed only
//! while the position differs from the target by more than [`SETTLE_THRESHOLD`].
//!
//! Time is the application's motion clock ([`vespertine::MotionClock::now_ms`]), so tests and
//! automation can sample motion deterministically.

use std::time::Duration;

use vespertine::{App, MotionClock, Window};

/// Duration of the switch thumb's travel.
pub const SWITCH_THUMB_DURATION: Duration = Duration::from_millis(180);

/// The thumb counts as arrived when it is this close to its target.
pub const SETTLE_THRESHOLD: f32 = 0.001;

/// Ease-out-cubic: `1 - (1 - t)^3` for `t` in `[0, 1]` (clamped).
pub fn ease_out_cubic(t: f32) -> f32 {
    let remaining = 1.0 - t.clamp(0.0, 1.0);
    1.0 - remaining * remaining * remaining
}

/// A scalar that glides to its target.
#[derive(Clone, Debug, PartialEq)]
pub struct ScalarMotion {
    from: f32,
    target: f32,
    start_ms: f64,
    duration_ms: f64,
}

impl ScalarMotion {
    /// A motion that starts settled at `target`: on first appearance nothing animates.
    pub fn new(target: f32, duration: Duration) -> Self {
        Self {
            from: target,
            target,
            start_ms: 0.0,
            duration_ms: duration.as_secs_f64() * 1000.0,
        }
    }

    /// The value the motion travels to.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// The duration of a full travel.
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.duration_ms / 1000.0)
    }

    /// Sets the duration of travels started from now on.
    pub fn set_duration(&mut self, duration: Duration) {
        self.duration_ms = duration.as_secs_f64() * 1000.0;
    }

    /// The position at `now_ms` (motion clock milliseconds).
    pub fn position(&self, now_ms: f64) -> f32 {
        if self.from == self.target {
            return self.target;
        }
        let progress = if self.duration_ms <= 0.0 {
            1.0
        } else {
            ((now_ms - self.start_ms) / self.duration_ms).clamp(0.0, 1.0) as f32
        };
        self.from + (self.target - self.from) * ease_out_cubic(progress)
    }

    /// Whether the position at `now_ms` is within [`SETTLE_THRESHOLD`] of the target. A frame
    /// must be requested exactly while this is false.
    pub fn is_settled(&self, now_ms: f64) -> bool {
        (self.position(now_ms) - self.target).abs() <= SETTLE_THRESHOLD
    }

    /// Moves the target. The travel restarts from the position at `now_ms`; with
    /// `reduced_motion` the position snaps to the target instead. Setting the current target
    /// again changes nothing (unless reduced motion asks for the snap).
    pub fn retarget(&mut self, target: f32, now_ms: f64, reduced_motion: bool) {
        if reduced_motion {
            self.snap(target);
        } else if target != self.target {
            self.from = self.position(now_ms);
            self.target = target;
            self.start_ms = now_ms;
        }
    }

    /// Jumps to `target` without travel.
    pub fn snap(&mut self, target: f32) {
        self.from = target;
        self.target = target;
    }

    /// Retargets with the application's motion clock and reduced-motion flag, returns the
    /// current position and requests the next frame while the position has not settled.
    pub fn advance(&mut self, target: f32, window: &Window, cx: &App) -> f32 {
        let now = MotionClock::now_ms(cx);
        self.retarget(target, now, cx.reduce_motion());
        let position = self.position(now);
        if !self.is_settled(now) {
            window.request_animation_frame();
        }
        position
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion(target: f32) -> ScalarMotion {
        ScalarMotion::new(target, SWITCH_THUMB_DURATION)
    }

    #[test]
    fn first_appearance_equals_the_target_without_animation() {
        for target in [0.0, 1.0] {
            let motion = motion(target);
            assert_eq!(motion.position(0.0), target);
            assert!(motion.is_settled(0.0));
        }
    }

    #[test]
    fn travel_follows_ease_out_cubic_over_180_ms() {
        let mut motion = motion(0.0);
        motion.retarget(1.0, 1000.0, false);
        assert_eq!(motion.position(1000.0), 0.0);
        for (elapsed, expected) in [(18.0, 0.271), (45.0, 0.578125), (90.0, 0.875), (180.0, 1.0)] {
            let position = motion.position(1000.0 + elapsed);
            assert!(
                (position - expected).abs() < 1e-4,
                "t = {elapsed} ms: {position} vs {expected}"
            );
        }
        assert_eq!(motion.position(5000.0), 1.0, "clamped past the end");
        assert!(!motion.is_settled(1000.0));
        assert!(!motion.is_settled(1100.0));
        assert!(motion.is_settled(1180.0));
    }

    #[test]
    fn retarget_during_travel_starts_from_the_current_position() {
        let mut motion = motion(0.0);
        motion.retarget(1.0, 0.0, false);
        let mid = motion.position(45.0);
        assert!((mid - 0.578125).abs() < 1e-4);
        motion.retarget(0.0, 45.0, false);
        assert_eq!(
            motion.position(45.0),
            mid,
            "no jump at the retarget instant"
        );
        let later = motion.position(45.0 + 90.0);
        let expected = mid + (0.0 - mid) * ease_out_cubic(0.5);
        assert!((later - expected).abs() < 1e-5);
        assert_eq!(motion.position(45.0 + 180.0), 0.0);
    }

    #[test]
    fn setting_the_same_target_does_not_restart_the_travel() {
        let mut motion = motion(0.0);
        motion.retarget(1.0, 0.0, false);
        let before = motion.position(90.0);
        motion.retarget(1.0, 90.0, false);
        assert_eq!(motion.position(90.0), before);
        assert_eq!(motion.position(180.0), 1.0);
    }

    #[test]
    fn reduced_motion_snaps_to_the_target_at_once() {
        let mut motion = motion(0.0);
        motion.retarget(1.0, 0.0, true);
        assert_eq!(motion.position(0.0), 1.0);
        assert!(motion.is_settled(0.0));
        // Reduction that starts mid-flight snaps too.
        motion.retarget(0.0, 10.0, false);
        assert!(!motion.is_settled(20.0));
        motion.retarget(0.0, 20.0, true);
        assert_eq!(motion.position(20.0), 0.0);
        assert!(motion.is_settled(20.0));
    }

    #[test]
    fn settled_means_within_a_thousandth_of_the_target() {
        let mut motion = motion(0.0);
        motion.retarget(1.0, 0.0, false);
        // 1 - (1 - t)^3 > 0.999 once (1 - t)^3 < 0.001, i.e. t > 0.9.
        assert!(!motion.is_settled(0.9 * 180.0 - 1.0));
        assert!(motion.is_settled(0.9 * 180.0 + 1.0));
    }
}
