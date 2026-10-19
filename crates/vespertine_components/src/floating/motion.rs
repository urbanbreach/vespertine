//! Entrance and exit motion of floating surfaces (CMP-002 R16).
//!
//! Entrance: 140 ms fade plus a 2 px settle (the surface starts 2 px out of the trigger side and
//! moves to rest); side-opening menus travel 4 px. Exit: 100 ms, eased, retreating along the
//! same travel. Progress is always computed from the motion-clock time since the phase began, so
//! it is monotonic and never replays from 0 mid-exit. With reduced motion (ANM-004,
//! [`vespertine::App::reduce_motion`]) both collapse to zero and the reap fires at once.

use super::lifecycle::Popup;
use std::time::Duration;
use vespertine::{App, MotionClock, Pixels, Point, point, px};

/// Entrance duration in milliseconds.
pub const ENTER_MS: f64 = 140.;
/// Exit duration in milliseconds.
pub const EXIT_MS: f64 = 100.;
/// Extra time after the exit span before the reap fires, in milliseconds.
pub const REAP_SLACK_MS: f64 = 20.;
/// Settle travel of a surface that opens below or above its trigger.
pub const SETTLE_TRAVEL: Pixels = px(2.);
/// Travel of a surface that opens beside its trigger.
pub const SIDE_TRAVEL: Pixels = px(4.);

/// The side of its trigger a floating surface sits on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    /// Above the trigger.
    Top,
    /// Right of the trigger.
    Right,
    /// Below the trigger (default).
    #[default]
    Bottom,
    /// Left of the trigger.
    Left,
}

impl Side {
    /// Travel of the entrance/exit: 2 px above and below the trigger, 4 px beside it.
    pub fn travel(self) -> Pixels {
        match self {
            Side::Top | Side::Bottom => SETTLE_TRAVEL,
            Side::Left | Side::Right => SIDE_TRAVEL,
        }
    }

    /// Unit vector from the surface's rest position toward its trigger.
    pub fn toward_trigger(self) -> (f32, f32) {
        match self {
            Side::Top => (0., 1.),
            Side::Bottom => (0., -1.),
            Side::Left => (1., 0.),
            Side::Right => (-1., 0.),
        }
    }
}

/// Delay of the reap scheduled by a close: exit span plus 20 ms, zero with reduced motion.
pub fn reap_delay(reduced_motion: bool) -> Duration {
    if reduced_motion {
        Duration::ZERO
    } else {
        Duration::from_secs_f64((EXIT_MS + REAP_SLACK_MS) / 1000.)
    }
}

/// Eased progress (0..=1) of the entrance that began at `opened_at`. Ease-out cubic.
pub fn enter_progress(now_ms: f64, opened_at: f64, reduced_motion: bool) -> f32 {
    if reduced_motion {
        return 1.;
    }
    let t = ((now_ms - opened_at) / ENTER_MS).clamp(0., 1.) as f32;
    1. - (1. - t).powi(3)
}

/// Eased progress (0..=1) of the exit that began at `since`. Ease-in cubic. Computed from the
/// wall-clock time since the close began, so a repaint mid-exit never replays it from 0.
pub fn exit_progress(now_ms: f64, since: f64, reduced_motion: bool) -> f32 {
    if reduced_motion {
        return 1.;
    }
    let t = ((now_ms - since) / EXIT_MS).clamp(0., 1.) as f32;
    t * t * t
}

/// What one frame of a surface's motion looks like.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceMotion {
    /// Opacity multiplier of the whole surface.
    pub opacity: f32,
    /// Displacement from the rest position.
    pub offset: Point<Pixels>,
    /// The surface is in its exit phase: hit testing is disabled and an occluding overlay
    /// swallows pointer events.
    pub dying: bool,
    /// Another frame is needed to finish the animation.
    pub animating: bool,
}

impl SurfaceMotion {
    /// The resting surface.
    pub const REST: Self = Self {
        opacity: 1.,
        offset: Point {
            x: px(0.),
            y: px(0.),
        },
        dying: false,
        animating: false,
    };

    /// [`SurfaceMotion::of`] at the application's motion clock, honoring reduced motion.
    pub fn now<T>(popup: &Popup<T>, side: Side, cx: &App) -> Self {
        Self::of(popup, MotionClock::now_ms(cx), side, cx.reduce_motion())
    }

    /// The motion of `popup` at motion time `now_ms` for a surface that sits on `side` of its
    /// trigger. `travel` is [`SETTLE_TRAVEL`] or [`SIDE_TRAVEL`] (see [`Side::travel`]).
    pub fn of<T>(popup: &Popup<T>, now_ms: f64, side: Side, reduced_motion: bool) -> Self {
        let travel = side.travel();
        // Unit vector pointing from the rest position toward the trigger.
        let (ux, uy) = side.toward_trigger();
        let at = |remaining: f32| {
            let distance = f32::from(travel) * remaining;
            point(px(ux * distance), px(uy * distance))
        };
        if let Some(since) = popup.closing_since() {
            let progress = exit_progress(now_ms, since, reduced_motion);
            Self {
                opacity: 1. - progress,
                offset: at(progress),
                dying: true,
                animating: progress < 1.,
            }
        } else if let Some(opened_at) = popup.opened_at() {
            let progress = enter_progress(now_ms, opened_at, reduced_motion);
            Self {
                opacity: progress,
                offset: at(1. - progress),
                dying: false,
                animating: progress < 1.,
            }
        } else {
            Self::REST
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reap_delay_is_exit_span_plus_slack_or_zero() {
        assert_eq!(reap_delay(false), Duration::from_millis(120));
        assert_eq!(reap_delay(true), Duration::ZERO);
    }

    #[test]
    fn progress_is_monotonic_and_clamped() {
        let mut previous = 0.;
        for step in 0..=20 {
            let p = exit_progress(step as f64 * 10., 0., false);
            assert!(p >= previous);
            previous = p;
        }
        assert_eq!(exit_progress(100., 0., false), 1.);
        assert_eq!(exit_progress(-5., 0., false), 0.);
        assert_eq!(enter_progress(140., 0., false), 1.);
        assert_eq!(enter_progress(0., 0., false), 0.);
    }

    #[test]
    fn exit_progress_depends_only_on_the_close_start() {
        // A repaint at the same wall-clock time yields the same progress: nothing replays.
        assert_eq!(
            exit_progress(60., 10., false),
            exit_progress(60., 10., false)
        );
        assert!(exit_progress(60., 10., false) > exit_progress(30., 10., false));
    }

    #[test]
    fn reduced_motion_collapses_both_phases() {
        assert_eq!(enter_progress(0., 0., true), 1.);
        assert_eq!(exit_progress(0., 0., true), 1.);
        let mut popup = Popup::new();
        popup.open((), 0.);
        let enter = SurfaceMotion::of(&popup, 0., Side::Bottom, true);
        assert_eq!(enter, SurfaceMotion::REST);
        popup.begin_close(1.);
        let exit = SurfaceMotion::of(&popup, 1., Side::Bottom, true);
        assert!(exit.dying && !exit.animating && exit.opacity == 0.);
    }

    #[test]
    fn entrance_settles_two_pixels_toward_the_trigger_and_side_menus_four() {
        let mut popup = Popup::new();
        popup.open((), 0.);
        let below = SurfaceMotion::of(&popup, 0., Side::Bottom, false);
        assert_eq!((below.opacity, below.offset), (0., point(px(0.), px(-2.))));
        assert!(below.animating && !below.dying);
        let right = SurfaceMotion::of(&popup, 0., Side::Right, false);
        assert_eq!(right.offset, point(px(-4.), px(0.)));
        let above = SurfaceMotion::of(&popup, 0., Side::Top, false);
        assert_eq!(above.offset, point(px(0.), px(2.)));
        let rest = SurfaceMotion::of(&popup, 140., Side::Bottom, false);
        assert_eq!(rest, SurfaceMotion::REST);
    }

    #[test]
    fn exit_retreats_along_the_entrance_travel() {
        let mut popup = Popup::new();
        popup.open((), 0.);
        popup.begin_close(200.);
        let start = SurfaceMotion::of(&popup, 200., Side::Bottom, false);
        assert!(start.dying && start.opacity == 1. && start.offset == point(px(0.), px(0.)));
        let end = SurfaceMotion::of(&popup, 300., Side::Bottom, false);
        assert!(end.opacity == 0. && end.offset == point(px(0.), px(-2.)) && !end.animating);
    }
}
