//! Edge autoscroll of virtualized lists during a live selection drag (TXT-008 R15).

use std::rc::Rc;
use std::time::Duration;
use vespertine::{Bounds, Pixels, Point, px};

/// Interval between autoscroll ticks.
pub const TICK: Duration = Duration::from_millis(24);

/// Largest step per tick, in logical pixels.
pub const MAX_STEP: f32 = 24.0;

/// Width of the edge band for a viewport of `height` logical px: `min(36, height / 6)`.
pub fn edge_band(height: f32) -> f32 {
    (height / 6.0).min(36.0)
}

/// Scroll step per tick for a pointer at `y` over a viewport spanning `top..bottom`: `24 x
/// progress^2`, negative (scroll up) in the top band and positive in the bottom band, 0 outside
/// both bands (a zero step schedules no tick).
pub fn step_for(y: f32, top: f32, bottom: f32) -> f32 {
    let edge = edge_band(bottom - top);
    if edge <= 0.0 {
        return 0.0;
    }
    let progress = |penetration: f32| (penetration / edge).clamp(0.0, 1.0);
    let top_penetration = top + edge - y;
    let bottom_penetration = y - (bottom - edge);
    if top_penetration > 0.0 {
        let p = progress(top_penetration);
        -MAX_STEP * p * p
    } else if bottom_penetration > 0.0 {
        let p = progress(bottom_penetration);
        MAX_STEP * p * p
    } else {
        0.0
    }
}

/// A scrollable list that can follow a selection drag; implemented by the virtual list.
pub trait ScrollTarget {
    /// Scrolls by `delta` (positive scrolls content up, towards later rows). Returns whether the
    /// scroll position changed.
    fn scroll_by(&self, delta: Pixels) -> bool;
}

/// A list registered while painting: its identity, viewport and scroll handle.
#[derive(Clone)]
pub struct RegisteredList {
    /// Identity of the list.
    pub id: u64,
    /// Viewport in window coordinates.
    pub viewport: Bounds<Pixels>,
    /// Scroll handle.
    pub target: Rc<dyn ScrollTarget>,
}

/// Picks the list that owns autoscroll for the pointer: the list whose viewport contains it; a
/// list that already owns the drag keeps ownership while the pointer stays within its horizontal
/// extent, even if the pointer leaves vertically.
pub fn pick_owner(
    lists: &[RegisteredList],
    current: Option<u64>,
    pointer: Point<Pixels>,
) -> Option<u64> {
    if let Some(current) = current
        && let Some(list) = lists.iter().find(|list| list.id == current)
        && pointer.x >= list.viewport.left()
        && pointer.x <= list.viewport.right()
    {
        return Some(current);
    }
    lists
        .iter()
        .rev()
        .find(|list| list.viewport.contains(&pointer))
        .map(|list| list.id)
}

/// What a tick found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickOutcome {
    /// Schedule another tick.
    Continue,
    /// Stop: the drag ended, the pointer left every list, or nothing moved.
    Stop,
}

/// The autoscroll controller: tracks the pointer and the owning list and exposes its tick and
/// step functions.
#[derive(Default)]
pub struct Autoscroll {
    owner: Option<u64>,
    pointer: Option<Point<Pixels>>,
    ticking: bool,
}

impl Autoscroll {
    /// Records the pointer of a live drag and returns the step the owning list should scroll per
    /// tick (0 when no tick should be scheduled).
    pub fn track(&mut self, lists: &[RegisteredList], pointer: Point<Pixels>) -> f32 {
        self.pointer = Some(pointer);
        self.owner = pick_owner(lists, self.owner, pointer);
        self.step(lists)
    }

    /// The step for the current owner and pointer.
    pub fn step(&self, lists: &[RegisteredList]) -> f32 {
        let (Some(owner), Some(pointer)) = (self.owner, self.pointer) else {
            return 0.0;
        };
        lists
            .iter()
            .find(|list| list.id == owner)
            .map_or(0.0, |list| {
                step_for(
                    f32::from(pointer.y),
                    f32::from(list.viewport.top()),
                    f32::from(list.viewport.bottom()),
                )
            })
    }

    /// Whether a tick loop is scheduled.
    pub fn is_ticking(&self) -> bool {
        self.ticking
    }

    /// Marks the loop scheduled; returns `false` when it already was.
    pub fn begin_ticking(&mut self) -> bool {
        !std::mem::replace(&mut self.ticking, true)
    }

    /// The tracked pointer.
    pub fn pointer(&self) -> Option<Point<Pixels>> {
        self.pointer
    }

    /// Ends the drag: forgets pointer and owner and stops the loop.
    pub fn finish(&mut self) {
        self.owner = None;
        self.pointer = None;
        self.ticking = false;
    }

    /// One tick (R15): re-resolve the spans at the tracked pointer (`reresolve` returns whether
    /// the selection changed), scroll the owning list by the step (`scroll` returns whether the
    /// list moved) and report whether to continue. Stops when the drag ended, the step is 0 (the
    /// pointer left the bands or every list), the list disappeared, or neither the selection nor
    /// the scroll position changed.
    pub fn tick(
        &mut self,
        lists: &[RegisteredList],
        drag_live: bool,
        reresolve: impl FnOnce(Point<Pixels>) -> bool,
    ) -> TickOutcome {
        let outcome = self.tick_inner(lists, drag_live, reresolve);
        if outcome == TickOutcome::Stop {
            self.ticking = false;
        }
        outcome
    }

    fn tick_inner(
        &mut self,
        lists: &[RegisteredList],
        drag_live: bool,
        reresolve: impl FnOnce(Point<Pixels>) -> bool,
    ) -> TickOutcome {
        if !drag_live {
            return TickOutcome::Stop;
        }
        let Some(pointer) = self.pointer else {
            return TickOutcome::Stop;
        };
        self.owner = pick_owner(lists, self.owner, pointer);
        let step = self.step(lists);
        if step == 0.0 {
            return TickOutcome::Stop;
        }
        let Some(list) = self
            .owner
            .and_then(|owner| lists.iter().find(|list| list.id == owner))
        else {
            return TickOutcome::Stop;
        };
        let selection_changed = reresolve(pointer);
        let list_moved = list.target.scroll_by(px(step));
        if selection_changed || list_moved {
            TickOutcome::Continue
        } else {
            TickOutcome::Stop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use vespertine::{point, size};

    #[test]
    fn edge_band_is_a_sixth_of_the_viewport_capped_at_36() {
        assert_eq!(edge_band(300.0), 36.0);
        assert_eq!(edge_band(120.0), 20.0);
        assert_eq!(edge_band(1000.0), 36.0);
    }

    #[test]
    fn steps_follow_the_quadratic_ramp() {
        // 300 px viewport: edge 36. 18 px into the bottom band: 24 * 0.25.
        assert_eq!(step_for(300.0 - 18.0, 0.0, 300.0), 6.0);
        assert_eq!(step_for(300.0 - 36.0, 0.0, 300.0), 0.0, "band boundary");
        assert_eq!(step_for(300.0, 0.0, 300.0), 24.0);
        assert_eq!(step_for(340.0, 0.0, 300.0), 24.0, "beyond the band clamps");
        assert_eq!(step_for(18.0, 0.0, 300.0), -6.0, "top band scrolls up");
        assert_eq!(step_for(-5.0, 0.0, 300.0), -24.0);
        assert_eq!(step_for(150.0, 0.0, 300.0), 0.0, "outside both bands");
        // 120 px viewport: edge 20; 10 px into the band is progress 0.5.
        assert_eq!(step_for(120.0 - 10.0, 0.0, 120.0), 6.0);
        // Offset viewports.
        assert_eq!(step_for(100.0 + 300.0 - 18.0, 100.0, 400.0), 6.0);
    }

    struct Scroller {
        position: Cell<f32>,
        max: f32,
        calls: Cell<usize>,
    }

    impl ScrollTarget for Scroller {
        fn scroll_by(&self, delta: Pixels) -> bool {
            self.calls.set(self.calls.get() + 1);
            let next = (self.position.get() + f32::from(delta)).clamp(0.0, self.max);
            let moved = next != self.position.get();
            self.position.set(next);
            moved
        }
    }

    fn list(id: u64, x: f32, y: f32, w: f32, h: f32, scroller: &Rc<Scroller>) -> RegisteredList {
        RegisteredList {
            id,
            viewport: Bounds {
                origin: point(px(x), px(y)),
                size: size(px(w), px(h)),
            },
            target: scroller.clone(),
        }
    }

    fn scroller(max: f32) -> Rc<Scroller> {
        Rc::new(Scroller {
            position: Cell::new(0.0),
            max,
            calls: Cell::new(0),
        })
    }

    #[test]
    fn ownership_follows_the_pointer_and_sticks_horizontally() {
        let s = scroller(100.0);
        let lists = [
            list(1, 0., 0., 200., 300., &s),
            list(2, 300., 0., 200., 300., &s),
        ];
        assert_eq!(pick_owner(&lists, None, point(px(10.), px(10.))), Some(1));
        assert_eq!(pick_owner(&lists, None, point(px(350.), px(10.))), Some(2));
        // Owning list keeps the drag when the pointer leaves vertically.
        assert_eq!(
            pick_owner(&lists, Some(1), point(px(10.), px(500.))),
            Some(1)
        );
        // ...but not when it leaves horizontally.
        assert_eq!(
            pick_owner(&lists, Some(1), point(px(350.), px(10.))),
            Some(2)
        );
        assert_eq!(pick_owner(&lists, None, point(px(250.), px(10.))), None);
    }

    #[test]
    fn no_tick_is_scheduled_for_a_zero_step() {
        let s = scroller(100.0);
        let lists = [list(1, 0., 0., 200., 300., &s)];
        let mut autoscroll = Autoscroll::default();
        assert_eq!(autoscroll.track(&lists, point(px(10.), px(150.))), 0.0);
        assert_eq!(autoscroll.track(&lists, point(px(10.), px(282.))), 6.0);
        let step = autoscroll.track(&lists, point(px(10.), px(299.)));
        assert!((step - 24.0 * (35.0f32 / 36.0).powi(2)).abs() < 1e-4);
    }

    #[test]
    fn ticks_scroll_and_stop_when_nothing_moves() {
        let s = scroller(30.0);
        let lists = [list(1, 0., 0., 200., 300., &s)];
        let mut autoscroll = Autoscroll::default();
        autoscroll.track(&lists, point(px(10.), px(299.)));
        assert!(autoscroll.begin_ticking());
        assert!(!autoscroll.begin_ticking(), "one loop at a time");
        let mut reresolved = 0;
        // First tick scrolls ~22.7; second reaches the end; third moves nothing.
        assert_eq!(
            autoscroll.tick(&lists, true, |_| {
                reresolved += 1;
                false
            }),
            TickOutcome::Continue
        );
        assert_eq!(
            autoscroll.tick(&lists, true, |_| {
                reresolved += 1;
                false
            }),
            TickOutcome::Continue
        );
        assert_eq!(
            autoscroll.tick(&lists, true, |_| {
                reresolved += 1;
                false
            }),
            TickOutcome::Stop
        );
        assert_eq!(s.position.get(), 30.0);
        assert_eq!(reresolved, 3);
        assert!(!autoscroll.is_ticking());
    }

    #[test]
    fn a_changed_selection_keeps_the_loop_alive() {
        let s = scroller(0.0);
        let lists = [list(1, 0., 0., 200., 300., &s)];
        let mut autoscroll = Autoscroll::default();
        autoscroll.track(&lists, point(px(10.), px(299.)));
        assert_eq!(
            autoscroll.tick(&lists, true, |_| true),
            TickOutcome::Continue
        );
    }

    #[test]
    fn the_loop_stops_when_the_drag_ends_or_the_pointer_leaves() {
        let s = scroller(100.0);
        let lists = [list(1, 0., 0., 200., 300., &s)];
        let mut autoscroll = Autoscroll::default();
        autoscroll.track(&lists, point(px(10.), px(299.)));
        assert_eq!(autoscroll.tick(&lists, false, |_| true), TickOutcome::Stop);
        autoscroll.track(&lists, point(px(10.), px(299.)));
        autoscroll.track(&lists, point(px(900.), px(300.)));
        assert_eq!(autoscroll.tick(&lists, true, |_| true), TickOutcome::Stop);
        assert_eq!(
            autoscroll.tick(&[], true, |_| true),
            TickOutcome::Stop,
            "list disappeared"
        );
        autoscroll.finish();
        assert!(autoscroll.pointer().is_none());
    }
}
