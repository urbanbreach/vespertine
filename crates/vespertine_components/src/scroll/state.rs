//! The state of one overlay rail: visibility policy, press/drag bookkeeping and the hide
//! scheduler (CMP-031 R10-R13).
//!
//! All times are milliseconds on one monotonic clock (`MotionClock::now_ms` in a window), passed
//! in explicitly so tests drive a controlled clock.

use super::metrics::{RAIL_THUMB_ACTIVE_THICKNESS, RAIL_THUMB_THICKNESS, RailMetrics};

/// How long the rail stays fully visible after the last scroll activity.
pub const RAIL_LINGER_MS: f64 = 1400.0;
/// How long the rail fades after the linger.
pub const RAIL_FADE_MS: f64 = 260.0;
/// The wake-up period while fading, so each intermediate opacity is painted.
pub const RAIL_FADE_WAKE_MS: f64 = 16.0;
/// Thumb opacity at rest (of the faint foreground).
pub const RAIL_OPACITY_REST: f32 = 0.50;
/// Thumb opacity while the strip is hovered or a drag is in progress.
pub const RAIL_OPACITY_ACTIVE: f32 = 0.68;
/// Thumb opacity while the pointer is directly over the thumb.
pub const RAIL_OPACITY_THUMB_HOVER: f32 = 0.85;

/// A position change below this many pixels is not scrolling (floating point noise).
const POSITION_EPSILON: f32 = 1e-3;

/// When a rail shows itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RailReveal {
    /// Vertical rails by default: shown by scroll activity, then lingering and fading; kept open
    /// while the strip is hovered or dragged. Merely hovering the list reveals nothing.
    #[default]
    Activity,
    /// Nested horizontal planes: visible while the plane's viewport is hovered or a drag is in
    /// progress, with no fade timer.
    Hover,
}

/// What the painter draws for the thumb right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RailLook {
    /// Thumb thickness: 3 px at rest, 5 px while the strip is active.
    pub thickness: f32,
    /// Opacity of the faint foreground: 0.50, 0.68 or 0.85, times the fade.
    pub opacity: f32,
}

/// The state of one rail. One object per rail; mutually exclusive popups may share one, and the
/// view that renders the list owns it.
#[derive(Clone, Debug, Default)]
pub struct RailState {
    reveal: RailReveal,
    list_hovered: bool,
    bar_hovered: bool,
    thumb_hovered: bool,
    grab_offset: Option<f32>,
    last_position: Option<f32>,
    last_activity_ms: Option<f64>,
    pending_wake_ms: Option<f64>,
    /// The painted strip's leading edge in window coordinates along the axis (the viewport's
    /// leading edge), recorded at paint time for pointer mapping.
    strip_origin: f32,
}

impl RailState {
    /// A rail with the activity policy of vertical rails.
    pub fn new() -> Self {
        Self::default()
    }

    /// A rail with the hover-reveal policy of nested horizontal planes (R14).
    pub fn hover_reveal() -> Self {
        Self {
            reveal: RailReveal::Hover,
            ..Self::default()
        }
    }

    /// The reveal policy.
    pub fn reveal(&self) -> RailReveal {
        self.reveal
    }

    /// Observes the scroll position at render time. The first observation only establishes a
    /// baseline (a fresh mount reports its initial offset, which is not scrolling); a later
    /// change of the position marks the activity time (R12).
    pub fn note_scroll(&mut self, position: f32, now_ms: f64) {
        match self.last_position {
            None => self.last_position = Some(position),
            Some(previous) if (previous - position).abs() > POSITION_EPSILON => {
                self.last_position = Some(position);
                self.last_activity_ms = Some(now_ms);
            }
            Some(_) => {}
        }
    }

    /// Forgets the baseline and the activity (switching the scroll state to another page or
    /// document): the next observed position is a baseline again and does not look like
    /// scrolling (R12).
    pub fn clear_scroll_baseline(&mut self) {
        self.last_position = None;
        self.last_activity_ms = None;
    }

    /// Whether a drag is in progress.
    pub fn dragging(&self) -> bool {
        self.grab_offset.is_some()
    }

    /// Whether the strip is hovered or captured by a drag: the rail's thumb is wide and its
    /// countdown is frozen (R10, R12).
    pub fn active(&self) -> bool {
        self.dragging() || self.bar_hovered
    }

    /// Whether the rail (and so its hit strip) exists right now (R8, R12).
    pub fn visible(&self, now_ms: f64) -> bool {
        if self.active() {
            return true;
        }
        match self.reveal {
            RailReveal::Hover => self.list_hovered,
            RailReveal::Activity => self.elapsed(now_ms).is_some_and(|e| e < Self::total_ms()),
        }
    }

    /// The opacity of the whole rail: 1 while hovered, dragged or lingering; then
    /// `clamp(1 - (elapsed - 1400) / 260, 0, 1)` through the fade; 0 once gone (R12).
    pub fn fade(&self, now_ms: f64) -> f32 {
        if self.active() {
            return 1.0;
        }
        match self.reveal {
            RailReveal::Hover => {
                if self.list_hovered {
                    1.0
                } else {
                    0.0
                }
            }
            RailReveal::Activity => match self.elapsed(now_ms) {
                Some(elapsed) if elapsed < RAIL_LINGER_MS => 1.0,
                Some(elapsed) if elapsed < Self::total_ms() => {
                    (1.0 - (elapsed - RAIL_LINGER_MS) / RAIL_FADE_MS).clamp(0.0, 1.0) as f32
                }
                _ => 0.0,
            },
        }
    }

    /// The thumb's look: thickness and opacity of the faint foreground (R10). Thumb hover affects
    /// only paint, never the strip.
    pub fn look(&self, now_ms: f64) -> RailLook {
        let active = self.active();
        let base = if self.thumb_hovered {
            RAIL_OPACITY_THUMB_HOVER
        } else if active {
            RAIL_OPACITY_ACTIVE
        } else {
            RAIL_OPACITY_REST
        };
        RailLook {
            thickness: if active {
                RAIL_THUMB_ACTIVE_THICKNESS
            } else {
                RAIL_THUMB_THICKNESS
            },
            opacity: base * self.fade(now_ms),
        }
    }

    /// The pointer entered or left the list (or plane). Leaving the list straight off the strip
    /// restarts the wait (R12).
    pub fn set_list_hovered(&mut self, hovered: bool, now_ms: f64) {
        if self.list_hovered == hovered {
            return;
        }
        self.list_hovered = hovered;
        if !hovered && self.bar_hovered {
            self.bar_hovered = false;
            self.thumb_hovered = false;
            self.restart_linger(now_ms);
        }
    }

    /// Whether the pointer is over the list (or plane).
    pub fn list_hovered(&self) -> bool {
        self.list_hovered
    }

    /// The pointer entered or left the strip. Hopping onto the strip mid-linger freezes the rail
    /// open; leaving it restarts the full wait so it never hides under the pointer (R12). A
    /// hidden rail has no strip, so it cannot be hovered.
    pub fn set_bar_hovered(&mut self, hovered: bool, now_ms: f64) {
        if hovered == self.bar_hovered {
            return;
        }
        if hovered {
            if !self.visible(now_ms) {
                return;
            }
            self.bar_hovered = true;
        } else {
            self.bar_hovered = false;
            self.thumb_hovered = false;
            self.restart_linger(now_ms);
        }
    }

    /// Whether the pointer is over the strip.
    pub fn bar_hovered(&self) -> bool {
        self.bar_hovered
    }

    /// The pointer is (not) directly over the thumb: paint only.
    pub fn set_thumb_hovered(&mut self, hovered: bool) {
        self.thumb_hovered = hovered && self.bar_hovered;
    }

    /// Whether the pointer is directly over the thumb.
    pub fn thumb_hovered(&self) -> bool {
        self.thumb_hovered
    }

    /// Starts a press on a rail with metrics `metrics`: `pointer` is the pointer's position in
    /// the track (window coordinate minus the track's start). Records the grab offset and returns
    /// the scroll fraction the drag target applies immediately, at press time (R11).
    pub fn begin_press_in(&mut self, metrics: &RailMetrics, pointer: f32) -> f32 {
        let grab = metrics.grab_offset(pointer);
        self.grab_offset = Some(grab);
        metrics.target_fraction(pointer, grab)
    }

    /// The scroll fraction a drag to `pointer` targets, or `None` when no drag is in progress.
    pub fn drag_target_in(&self, metrics: &RailMetrics, pointer: f32) -> Option<f32> {
        self.grab_offset
            .map(|grab| metrics.target_fraction(pointer, grab))
    }

    /// Ends the drag. Releasing counts as a stopped scroll: the rail lingers (R12).
    pub fn end_press(&mut self, now_ms: f64) {
        if self.grab_offset.take().is_some() {
            self.restart_linger(now_ms);
        }
    }

    /// The viewport's leading edge in window coordinates along the axis, as last painted.
    pub fn strip_origin(&self) -> f32 {
        self.strip_origin
    }

    /// Records the painted strip's leading edge.
    pub fn set_strip_origin(&mut self, origin: f32) {
        self.strip_origin = origin;
    }

    /// The pointer position in the track for a window coordinate along the axis.
    pub fn pointer_in_track(&self, window_coordinate: f32) -> f32 {
        window_coordinate - self.strip_origin - super::metrics::RAIL_TRACK_INSET
    }

    /// Arms the hide timer: returns the delay (ms) to wait before the next wake-up, or `None`
    /// when nothing is to be scheduled (R13). At most one wake is ever in flight: while one is
    /// pending this returns `None`. While a countdown runs, one wake is scheduled for the end of
    /// the linger and then every 16 ms through the fade; after the fade completes none. A rail
    /// with no countdown, drag or hover costs nothing.
    pub fn arm_hide_timer(&mut self, now_ms: f64) -> Option<f64> {
        if self.pending_wake_ms.is_some() || self.active() || self.reveal != RailReveal::Activity {
            return None;
        }
        let elapsed = self.elapsed(now_ms)?;
        let delay = if elapsed < RAIL_LINGER_MS {
            RAIL_LINGER_MS - elapsed
        } else if elapsed < Self::total_ms() {
            RAIL_FADE_WAKE_MS.min(Self::total_ms() - elapsed)
        } else {
            return None;
        };
        self.pending_wake_ms = Some(now_ms + delay);
        Some(delay)
    }

    /// The scheduled wake-up fired (or was dropped): the next render may arm another.
    pub fn wake_fired(&mut self) {
        self.pending_wake_ms = None;
    }

    /// Whether a wake-up is in flight.
    pub fn wake_pending(&self) -> bool {
        self.pending_wake_ms.is_some()
    }

    fn total_ms() -> f64 {
        RAIL_LINGER_MS + RAIL_FADE_MS
    }

    fn elapsed(&self, now_ms: f64) -> Option<f64> {
        self.last_activity_ms.map(|at| (now_ms - at).max(0.0))
    }

    fn restart_linger(&mut self, now_ms: f64) {
        if self.reveal == RailReveal::Activity {
            self.last_activity_ms = Some(now_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrolled(at: f64) -> RailState {
        let mut state = RailState::new();
        state.note_scroll(0.0, 0.0);
        state.note_scroll(10.0, at);
        state
    }

    #[test]
    fn a6_initial_mount_is_a_baseline_and_shows_nothing() {
        let mut state = RailState::new();
        state.note_scroll(120.0, 0.0);
        assert!(!state.visible(0.0));
        state.note_scroll(120.0, 100.0);
        assert!(!state.visible(100.0));
        assert_eq!(state.fade(100.0), 0.0);
    }

    #[test]
    fn a6_linger_fade_and_disappearance() {
        let state = scrolled(0.0);
        assert!(state.visible(0.0));
        assert_eq!(state.fade(0.0), 1.0);
        assert_eq!(state.fade(1400.0), 1.0);
        assert!((state.fade(1530.0) - 0.5).abs() < 1e-6);
        assert!(state.visible(1659.0));
        assert!(!state.visible(1660.0));
        assert_eq!(state.fade(1660.0), 0.0);
    }

    #[test]
    fn a6_hover_freezes_and_leaving_restarts_the_wait() {
        let mut state = scrolled(0.0);
        state.set_bar_hovered(true, 1000.0);
        assert_eq!(state.fade(1000.0), 1.0);
        assert_eq!(state.fade(30_000.0), 1.0);
        assert!(state.visible(30_000.0));
        state.set_bar_hovered(false, 30_000.0);
        assert_eq!(state.fade(31_399.0), 1.0);
        assert!(state.fade(31_500.0) < 1.0);
        assert!(!state.visible(31_660.0));
    }

    #[test]
    fn a6_leaving_the_list_off_the_strip_restarts_the_wait() {
        let mut state = scrolled(0.0);
        state.set_list_hovered(true, 500.0);
        state.set_bar_hovered(true, 1000.0);
        state.set_list_hovered(false, 5000.0);
        assert!(!state.bar_hovered());
        assert_eq!(state.fade(6399.0), 1.0);
        assert!(!state.visible(6660.0));
    }

    #[test]
    fn a6_hovering_an_idle_list_never_reveals_the_rail() {
        let mut state = RailState::new();
        state.note_scroll(0.0, 0.0);
        state.set_list_hovered(true, 10.0);
        assert!(!state.visible(10.0));
        // A hidden rail has no strip, so the strip cannot be hovered either.
        state.set_bar_hovered(true, 20.0);
        assert!(!state.bar_hovered());
        assert!(!state.visible(20.0));
    }

    #[test]
    fn a6_releasing_a_drag_lingers_and_clearing_the_baseline_forgets() {
        let mut state = scrolled(0.0);
        let metrics = RailMetrics::from_extents(320.0, 960.0, 0.0).unwrap();
        state.begin_press_in(&metrics, 50.0);
        assert!(state.dragging() && state.active());
        assert!(state.visible(9_999.0));
        state.end_press(10_000.0);
        assert!(!state.dragging());
        assert_eq!(state.fade(11_399.0), 1.0);
        assert!(!state.visible(11_660.0));

        // Switching pages: the next observation is a baseline, not scrolling.
        let mut state = scrolled(0.0);
        state.clear_scroll_baseline();
        assert!(!state.visible(1.0));
        state.note_scroll(300.0, 2.0);
        assert!(!state.visible(2.0));
        state.note_scroll(310.0, 3.0);
        assert!(state.visible(3.0));
    }

    #[test]
    fn a7_wake_schedule_has_one_timer_then_16_ms_steps_then_none() {
        let mut state = scrolled(0.0);
        assert_eq!(state.arm_hide_timer(0.0), Some(1400.0));
        // At most one wake in flight.
        assert_eq!(state.arm_hide_timer(1.0), None);
        assert!(state.wake_pending());
        state.wake_fired();
        let mut now = 1400.0;
        let mut wakes = Vec::new();
        while let Some(delay) = state.arm_hide_timer(now) {
            wakes.push(delay);
            now += delay;
            state.wake_fired();
        }
        assert_eq!(wakes.len(), 17);
        assert!(wakes[..16].iter().all(|delay| *delay == 16.0));
        assert_eq!(wakes[16], 4.0);
        assert_eq!(now, 1660.0);
        assert!(!state.visible(now));
        assert_eq!(state.arm_hide_timer(now), None);
    }

    #[test]
    fn a7_idle_hovered_or_dragged_rails_schedule_nothing() {
        let mut idle = RailState::new();
        idle.note_scroll(0.0, 0.0);
        assert_eq!(idle.arm_hide_timer(0.0), None);
        let mut hovered = scrolled(0.0);
        hovered.set_bar_hovered(true, 10.0);
        assert_eq!(hovered.arm_hide_timer(10.0), None);
        let mut plane = RailState::hover_reveal();
        plane.set_list_hovered(true, 0.0);
        assert!(plane.visible(0.0));
        assert_eq!(plane.arm_hide_timer(0.0), None);
        plane.set_list_hovered(false, 1.0);
        assert!(!plane.visible(1.0));
    }

    #[test]
    fn a8_thumb_thickness_and_opacity() {
        let mut state = scrolled(0.0);
        let rest = state.look(0.0);
        assert_eq!((rest.thickness, rest.opacity), (3.0, 0.50));
        state.set_bar_hovered(true, 10.0);
        let hovered = state.look(10.0);
        assert_eq!((hovered.thickness, hovered.opacity), (5.0, 0.68));
        state.set_thumb_hovered(true);
        assert_eq!(state.look(10.0).opacity, 0.85);
        state.set_thumb_hovered(false);
        state.set_bar_hovered(false, 20.0);

        let mut dragged = scrolled(0.0);
        let metrics = RailMetrics::from_extents(320.0, 960.0, 0.0).unwrap();
        dragged.begin_press_in(&metrics, 10.0);
        let look = dragged.look(5000.0);
        assert_eq!((look.thickness, look.opacity), (5.0, 0.68));
    }

    #[test]
    fn a5_a_drag_continues_outside_the_rail_and_ends_on_release() {
        let metrics = RailMetrics::from_extents(320.0, 960.0, 0.0).unwrap();
        let mut state = scrolled(0.0);
        assert_eq!(state.drag_target_in(&metrics, 10.0), None);
        let fraction = state.begin_press_in(&metrics, 40.0);
        assert!((fraction - 0.0).abs() < 1e-6);
        assert_eq!(state.drag_target_in(&metrics, -999.0), Some(0.0));
        assert_eq!(state.drag_target_in(&metrics, 99999.0), Some(1.0));
        state.end_press(100.0);
        assert_eq!(state.drag_target_in(&metrics, 100.0), None);
    }
}
