//! Submenu hover intent with a pointer corridor (CMP-002 R26, R27).
//!
//! [`HoverIntent`] decides when pointer motion over another trigger row switches the open
//! submenu. It owns only the state (origin, pending target, the pointer at the last renewal);
//! the owner owns the 300 ms timer ([`HOVER_GRACE`]): it starts the timer when an answer is
//! [`IntentAnswer::Defer`], and when the timer fires it calls [`HoverIntent::fire`], switching
//! only if that returns true and the parent and source submenu are still open.
//!
//! Submenus open on either side: every call that needs geometry takes the open submenu with its
//! `left` flag, which mirrors the horizontal sign.

use std::time::Duration;
use vespertine::{Bounds, Pixels, Point};

/// Grace period before a deferred switch happens.
pub const HOVER_GRACE: Duration = Duration::from_millis(300);
/// Forward travel (px) that renews the grace.
pub const RENEW_THRESHOLD: f32 = 2.;
/// Backward travel (px, as a negative bound) that counts as turning away.
pub const TURN_AWAY_THRESHOLD: f32 = -2.;
/// Corridor tolerance past the child's near edge and above/below the child.
pub const CORRIDOR_TOLERANCE: f32 = 8.;

/// What the owner does with a pointer event over a trigger row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntentAnswer {
    /// Nothing; an already running timer continues.
    None,
    /// Switch to the target immediately.
    Open,
    /// Start (or restart) the grace timer; switch when it fires and [`HoverIntent::fire`]
    /// confirms.
    Defer,
}

/// The submenu that is currently open, as far as hover intent needs to know.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenSubmenu<'a, K> {
    /// Key of the trigger row that owns the open submenu.
    pub key: &'a K,
    /// Window bounds of the child menu, when it has been laid out.
    pub child: Option<Bounds<Pixels>>,
    /// The child opens to the left of its trigger.
    pub left: bool,
}

/// Whether `pointer` lies in the corridor from `origin` toward `child` (R27).
///
/// `left` is true for children opening to the left. The corridor is a funnel from the origin
/// widening to the child's height plus 8 px above and below.
pub fn in_corridor(
    origin: Point<Pixels>,
    pointer: Point<Pixels>,
    child: Bounds<Pixels>,
    left: bool,
) -> bool {
    let direction = if left { -1. } else { 1. };
    let (origin_x, origin_y) = (f32::from(origin.x), f32::from(origin.y));
    let edge = if left {
        f32::from(child.right())
    } else {
        f32::from(child.left())
    };
    let distance = (edge - origin_x) * direction;
    let advance = (f32::from(pointer.x) - origin_x) * direction;
    if distance <= 0. || advance <= 0. || advance > distance + CORRIDOR_TOLERANCE {
        return false;
    }
    let fraction = (advance / distance).min(1.);
    let top = origin_y + (f32::from(child.top()) - CORRIDOR_TOLERANCE - origin_y) * fraction;
    let bottom = origin_y + (f32::from(child.bottom()) + CORRIDOR_TOLERANCE - origin_y) * fraction;
    let y = f32::from(pointer.y);
    top <= y && y <= bottom
}

/// Hover-intent state for one submenu level; `K` identifies a trigger row.
#[derive(Clone, Debug)]
pub struct HoverIntent<K> {
    origin: Option<Point<Pixels>>,
    pending: Option<K>,
    remembered: Option<Point<Pixels>>,
}

impl<K> Default for HoverIntent<K> {
    fn default() -> Self {
        Self {
            origin: None,
            pending: None,
            remembered: None,
        }
    }
}

impl<K: PartialEq + Clone> HoverIntent<K> {
    /// Fresh state: no origin, nothing pending.
    pub fn new() -> Self {
        Self::default()
    }

    /// The last pointer position recorded inside the active trigger row.
    pub fn origin(&self) -> Option<Point<Pixels>> {
        self.origin
    }

    /// Records the pointer as the origin.
    pub fn record_origin(&mut self, pointer: Point<Pixels>) {
        self.origin = Some(pointer);
    }

    /// The pending target, if a switch is deferred.
    pub fn pending(&self) -> Option<&K> {
        self.pending.as_ref()
    }

    /// Whether `target` is the pending one.
    pub fn is_pending(&self, target: &K) -> bool {
        self.pending.as_ref() == Some(target)
    }

    /// Pointer entered the row `target`.
    pub fn enter(
        &mut self,
        target: &K,
        pointer: Point<Pixels>,
        open: Option<OpenSubmenu<'_, K>>,
    ) -> IntentAnswer {
        self.cancel();
        let Some(open) = open else {
            return IntentAnswer::Open;
        };
        if open.key == target {
            self.record_origin(pointer);
            return IntentAnswer::None;
        }
        let toward_child = match (self.origin, open.child) {
            (Some(origin), Some(child)) => in_corridor(origin, pointer, child, open.left),
            _ => false,
        };
        if toward_child {
            self.pending = Some(target.clone());
            self.remembered = Some(pointer);
            IntentAnswer::Defer
        } else {
            IntentAnswer::Open
        }
    }

    /// Pointer moved over the row `target`.
    pub fn moved(
        &mut self,
        target: &K,
        pointer: Point<Pixels>,
        open: Option<OpenSubmenu<'_, K>>,
    ) -> IntentAnswer {
        if open.as_ref().is_some_and(|open| open.key == target) {
            self.record_origin(pointer);
            return IntentAnswer::None;
        }
        if !self.is_pending(target) {
            return IntentAnswer::None;
        }
        let (Some(open), Some(remembered), Some(origin)) = (open, self.remembered, self.origin)
        else {
            self.cancel();
            return IntentAnswer::Open;
        };
        let direction = if open.left { -1. } else { 1. };
        let forward = (f32::from(pointer.x) - f32::from(remembered.x)) * direction;
        let in_funnel = open
            .child
            .is_some_and(|child| in_corridor(origin, pointer, child, open.left));
        if !in_funnel || forward <= TURN_AWAY_THRESHOLD {
            self.cancel();
            return IntentAnswer::Open;
        }
        if forward >= RENEW_THRESHOLD {
            return self.enter(target, pointer, Some(open));
        }
        IntentAnswer::None
    }

    /// Pointer left the row `key`: only the pending target's leave cancels.
    pub fn leave(&mut self, key: &K) {
        if self.is_pending(key) {
            self.cancel();
        }
    }

    /// Click, keyboard or dismissal: drops the pending target.
    pub fn cancel(&mut self) {
        self.pending = None;
        self.remembered = None;
    }

    /// Drops the pending target and the origin.
    pub fn reset(&mut self) {
        self.cancel();
        self.origin = None;
    }

    /// The grace timer for `target` fired: true (and the pending state is consumed) only when
    /// `target` is still pending. The owner additionally checks that the parent and the source
    /// submenu are still open.
    pub fn fire(&mut self, target: &K) -> bool {
        if self.is_pending(target) {
            self.cancel();
            true
        } else {
            false
        }
    }

    /// Window pointer tracking for dismissal (R26): whether `pointer` is inside the safe region.
    ///
    /// Safe: inside the trigger row (the origin is recorded), no child exists, inside the child
    /// bounds, or inside the corridor. Anything else may dismiss the nested layer. Reads the
    /// position only; it does not capture the pointer.
    pub fn contains_pointer(
        &mut self,
        pointer: Point<Pixels>,
        trigger: Bounds<Pixels>,
        child: Option<Bounds<Pixels>>,
        left: bool,
    ) -> bool {
        if trigger.contains(&pointer) {
            self.record_origin(pointer);
            return true;
        }
        let Some(child) = child else {
            return true;
        };
        if child.contains(&pointer) {
            return true;
        }
        self.origin
            .is_some_and(|origin| in_corridor(origin, pointer, child, left))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{point, px, size};

    fn pt(x: f32, y: f32) -> Point<Pixels> {
        point(px(x), px(y))
    }

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(pt(x, y), size(px(w), px(h)))
    }

    /// Mirrors an x coordinate about x = 190 (the right-opening child at 200..280 becomes a
    /// left-opening child at 100..180).
    fn mirror(x: f32) -> f32 {
        380. - x
    }

    /// A10, for either opening side. Coordinates are given for the right-opening case and
    /// mirrored for the left-opening one.
    fn a10(left: bool) {
        let m = |x: f32| if left { mirror(x) } else { x };
        let child = if left {
            bounds(mirror(280.), 80., 80., 160.)
        } else {
            bounds(200., 80., 80., 160.)
        };
        let open = |key: &'static &str| OpenSubmenu {
            key,
            child: Some(child),
            left,
        };
        let current: &'static &str = &"current";
        let target: &'static &str = &"target";
        let other: &'static &str = &"other";
        let mut intent: HoverIntent<&str> = HoverIntent::new();
        intent.record_origin(pt(m(100.), 100.));
        assert_eq!(
            intent.enter(target, pt(m(150.), 140.), Some(open(current))),
            IntentAnswer::Defer
        );
        assert!(intent.is_pending(target));
        assert_eq!(
            intent.moved(target, pt(m(153.), 141.), Some(open(current))),
            IntentAnswer::Defer
        );
        assert_eq!(
            intent.moved(target, pt(m(153.5), 141.), Some(open(current))),
            IntentAnswer::None
        );
        assert!(intent.is_pending(target));
        // Leave of another key does not cancel; leave of the pending key does.
        intent.leave(other);
        assert!(intent.is_pending(target));
        assert_eq!(
            intent.moved(target, pt(m(151.), 141.), Some(open(current))),
            IntentAnswer::Open
        );
        assert_eq!(intent.pending(), None);

        intent.enter(target, pt(m(150.), 140.), Some(open(current)));
        intent.leave(target);
        assert_eq!(intent.pending(), None);

        // Reset clears the origin: the same enter then answers open.
        intent.enter(target, pt(m(150.), 140.), Some(open(current)));
        intent.reset();
        assert_eq!(intent.origin(), None);
        assert_eq!(intent.pending(), None);
        assert_eq!(
            intent.enter(target, pt(m(150.), 140.), Some(open(current))),
            IntentAnswer::Open
        );
    }

    #[test]
    fn a10_right_opening_vector() {
        a10(false);
    }

    #[test]
    fn a10_left_opening_gives_identical_answers() {
        a10(true);
    }

    #[test]
    fn fire_confirms_only_the_pending_target() {
        let key: &str = "a";
        let current: &str = "cur";
        let child = bounds(200., 80., 80., 160.);
        let mut intent: HoverIntent<&str> = HoverIntent::new();
        intent.record_origin(pt(100., 100.));
        let open = Some(OpenSubmenu {
            key: &current,
            child: Some(child),
            left: false,
        });
        assert_eq!(
            intent.enter(&key, pt(150., 140.), open),
            IntentAnswer::Defer
        );
        assert!(!intent.fire(&"b"));
        assert!(intent.fire(&key));
        assert!(!intent.fire(&key));
        // Cancel (click, keyboard, dismissal) makes a later fire do nothing.
        intent.enter(&key, pt(150., 140.), open);
        intent.cancel();
        assert!(!intent.fire(&key));
    }

    #[test]
    fn entering_the_open_submenu_row_only_records_the_origin() {
        let key: &str = "a";
        let mut intent: HoverIntent<&str> = HoverIntent::new();
        let open = Some(OpenSubmenu {
            key: &key,
            child: Some(bounds(200., 80., 80., 160.)),
            left: false,
        });
        assert_eq!(intent.enter(&key, pt(120., 90.), open), IntentAnswer::None);
        assert_eq!(intent.origin(), Some(pt(120., 90.)));
        assert_eq!(intent.moved(&key, pt(121., 91.), open), IntentAnswer::None);
        assert_eq!(intent.origin(), Some(pt(121., 91.)));
        // No submenu open: switch immediately.
        assert_eq!(intent.enter(&key, pt(0., 0.), None), IntentAnswer::Open);
    }

    #[test]
    fn leaving_the_corridor_cancels_and_opens() {
        let key: &str = "a";
        let current: &str = "cur";
        let open = Some(OpenSubmenu {
            key: &current,
            child: Some(bounds(200., 80., 80., 160.)),
            left: false,
        });
        let mut intent: HoverIntent<&str> = HoverIntent::new();
        intent.record_origin(pt(100., 100.));
        assert_eq!(
            intent.enter(&key, pt(150., 140.), open),
            IntentAnswer::Defer
        );
        // Forward motion but far outside the funnel.
        assert_eq!(intent.moved(&key, pt(152., 400.), open), IntentAnswer::Open);
        assert_eq!(intent.pending(), None);
    }

    /// A11, right-opening.
    #[test]
    fn a11_safe_region_vector() {
        let trigger = bounds(50., 85., 100., 30.);
        let child = bounds(200., 80., 80., 160.);
        let mut intent: HoverIntent<&str> = HoverIntent::new();
        assert!(intent.contains_pointer(pt(100., 100.), trigger, Some(child), false));
        assert_eq!(intent.origin(), Some(pt(100., 100.)));
        assert!(intent.contains_pointer(pt(175., 140.), trigger, Some(child), false));
        assert!(intent.contains_pointer(pt(240., 160.), trigger, Some(child), false));
        assert!(!intent.contains_pointer(pt(175., 300.), trigger, Some(child), false));
        // No child: always safe.
        assert!(intent.contains_pointer(pt(900., 900.), trigger, None, false));
        // An enter at (100, 140) with a submenu open: advance 0, answers open.
        let current: &str = "cur";
        let target: &str = "t";
        let open = Some(OpenSubmenu {
            key: &current,
            child: Some(child),
            left: false,
        });
        assert_eq!(
            intent.enter(&target, pt(100., 140.), open),
            IntentAnswer::Open
        );
    }

    #[test]
    fn a11_safe_region_mirrored() {
        let trigger = bounds(mirror(150.), 85., 100., 30.);
        let child = bounds(mirror(280.), 80., 80., 160.);
        let mut intent: HoverIntent<&str> = HoverIntent::new();
        assert!(intent.contains_pointer(pt(mirror(100.), 100.), trigger, Some(child), true));
        assert!(intent.contains_pointer(pt(mirror(175.), 140.), trigger, Some(child), true));
        assert!(intent.contains_pointer(pt(mirror(240.), 160.), trigger, Some(child), true));
        assert!(!intent.contains_pointer(pt(mirror(175.), 300.), trigger, Some(child), true));
    }

    #[test]
    fn corridor_geometry_edges() {
        let origin = pt(100., 100.);
        let child = bounds(200., 80., 80., 160.);
        // Behind the origin, or no advance: outside.
        assert!(!in_corridor(origin, pt(100., 100.), child, false));
        assert!(!in_corridor(origin, pt(90., 100.), child, false));
        // Beyond the child's near edge by more than the tolerance: outside.
        assert!(in_corridor(origin, pt(208., 100.), child, false));
        assert!(!in_corridor(origin, pt(208.5, 100.), child, false));
        // Origin at or past the near edge: outside.
        assert!(!in_corridor(pt(200., 100.), pt(210., 100.), child, false));
        // At the edge the funnel is the child's height plus 8 px.
        assert!(in_corridor(origin, pt(200., 72.), child, false));
        assert!(!in_corridor(origin, pt(200., 71.9), child, false));
        assert!(in_corridor(origin, pt(200., 248.), child, false));
        assert!(!in_corridor(origin, pt(200., 248.1), child, false));
    }
}
