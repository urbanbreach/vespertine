//! The follow controller (CMP-029): the policy layer over a variable-height virtual list that
//! decides when the view follows new content, glides with the velocity spring, anchors a freshly
//! sent item, keeps the bottom correct under chrome changes, and navigates explicitly.
//!
//! The controller never interprets a position change it caused itself as user intent: only the
//! list's user-initiated scroll notification (wheel and touch) reaches the policy (R2).

use std::{cell::RefCell, collections::VecDeque, ops::Range, rc::Rc};

use vespertine::{App, ListOffset, ListScrollEvent, ListState, MotionClock, Window, px};

use super::follow::{
    AT_BOTTOM_EPSILON, FollowPolicy, LANDING_SNAP, NAVIGATION_DURATION_MS, NavigationTimeline,
    PinChange, Stepper, autoscroll_step, list_viewport_height, reservation_changed,
    reserved_height, teleport_excess,
};

/// Slack under an anchored hold: 2 px of extra height under the reservation (R12).
const HOLD_SLACK: f32 = 2.0;
/// Tolerance added to the slack (R12).
const HOLD_TOLERANCE: f32 = 2.0;
/// Only drift above the hold beyond this is corrected (R12).
const HOLD_DRIFT: f32 = 0.5;
/// The entry glide retains this fraction of the error per 60 fps frame (R12).
const HOLD_RETAIN: f32 = 0.85;
/// The list's overdraw: steps up to 0.8 of it scroll relatively (R21).
const NAV_OVERDRAW: f32 = 320.0;
/// The pixel anchor materialized above the glued bottom (R8).
const GLUE_ANCHOR: f32 = 0.75;
/// Viewport memory is bounded (R16).
const MEMORY_ENTRIES: usize = 256;

/// A content commit applied to the list (R8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Commit {
    /// A row splice: `old_range` replaced by `new_count` rows.
    Splice {
        /// Replaced rows.
        old_range: Range<usize>,
        /// Replacement count.
        new_count: usize,
    },
    /// Rows replaced in place (same count, same identities, for example a streamed item
    /// finishing): re-measured retaining their old heights as hints, never spliced.
    ReplaceInPlace(Range<usize>),
}

/// The reader's remembered place of one conversation (R16).
#[derive(Clone, Debug, PartialEq)]
pub enum ViewportSnapshot<K> {
    /// Follow the tail (stored when pinned).
    FollowTail,
    /// An anchored viewport: the identity of the top item, the offset within it and the distance
    /// from the bottom.
    Anchored {
        /// Identity of the top item.
        item: K,
        /// Pixel offset within it.
        offset: f32,
        /// Distance from the bottom.
        distance: f32,
    },
}

/// Per-conversation viewport memory, bounded to 256 entries (the oldest evicted) (R16).
#[derive(Clone, Debug)]
pub struct ViewportMemory<C: Clone + Eq, K: Clone> {
    entries: VecDeque<(C, ViewportSnapshot<K>)>,
}

impl<C: Clone + Eq, K: Clone> Default for ViewportMemory<C, K> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}

impl<C: Clone + Eq, K: Clone> ViewportMemory<C, K> {
    /// Empty memory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Remembers the place of `conversation`. An empty list (`row_count == 0`) never overwrites
    /// an older snapshot.
    pub fn remember(&mut self, conversation: C, snapshot: ViewportSnapshot<K>, row_count: usize) {
        if row_count == 0 {
            return;
        }
        self.entries.retain(|(key, _)| *key != conversation);
        self.entries.push_back((conversation, snapshot));
        while self.entries.len() > MEMORY_ENTRIES {
            self.entries.pop_front();
        }
    }

    /// The remembered place of `conversation`.
    pub fn recall(&self, conversation: &C) -> Option<&ViewportSnapshot<K>> {
        self.entries
            .iter()
            .find(|(key, _)| key == conversation)
            .map(|(_, snapshot)| snapshot)
    }

    /// Retires a snapshot proved absent by an authoritative empty reset.
    pub fn retire(&mut self, conversation: &C) {
        self.entries.retain(|(key, _)| key != conversation);
    }

    /// Explicit user or navigation intent replaces a pending restore with follow-tail.
    pub fn replace_with_follow_tail(&mut self, conversation: C) {
        self.entries.retain(|(key, _)| *key != conversation);
        self.entries
            .push_back((conversation, ViewportSnapshot::FollowTail));
        while self.entries.len() > MEMORY_ENTRIES {
            self.entries.pop_front();
        }
    }

    /// The number of remembered conversations.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is remembered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HoldPhase {
    Entering,
    Holding,
    /// Released by a user scroll; re-armed when the user returns to the hold position moving
    /// toward it.
    Released,
}

#[derive(Clone, Copy, Debug)]
struct Hold {
    item: usize,
    inset: f32,
    phase: HoldPhase,
    last_ms: Option<f64>,
}

struct Navigation {
    row: usize,
    timeline: NavigationTimeline,
    items_ema: f32,
}

struct Inner {
    policy: FollowPolicy,
    stepper: Stepper,
    hold: Option<Hold>,
    navigation: Option<Navigation>,
    bottom_clearance: f32,
    right_reserve: f32,
    tail_streaming: bool,
    selection_drag: bool,
    kick: bool,
    frame_scheduled: bool,
    long_jump_done: bool,
    visible_range: Option<Rc<RefCell<dyn FnMut(&ListScrollEvent, &mut Window, &mut App)>>>,
}

/// The follow controller of one list. Cloning shares the controller.
#[derive(Clone)]
pub struct FollowController {
    inner: Rc<RefCell<Inner>>,
    list: ListState,
}

impl FollowController {
    /// Attaches a controller to `list` and installs its user-scroll handler. The list is
    /// scrolled to its end when it already has rows (a pinned conversation opens at the bottom).
    pub fn attach(list: ListState) -> Self {
        let controller = Self {
            inner: Rc::new(RefCell::new(Inner {
                policy: FollowPolicy::new(),
                stepper: Stepper::new(),
                hold: None,
                navigation: None,
                bottom_clearance: 0.0,
                right_reserve: 0.0,
                tail_streaming: false,
                selection_drag: false,
                kick: false,
                frame_scheduled: false,
                long_jump_done: false,
                visible_range: None,
            })),
            list,
        };
        controller
            .list
            .set_follow_mode(vespertine::FollowMode::Normal);
        if controller.list.item_count() > 0 {
            controller.list.scroll_to_end();
        }
        let handler = controller.clone();
        controller
            .list
            .set_scroll_handler(move |event, window, cx| handler.on_list_event(event, window, cx));
        controller
    }

    /// The list this controller drives.
    pub fn list(&self) -> &ListState {
        &self.list
    }

    /// Observes the list's visible-range notifications (R23 edge rows); user scroll events reach
    /// the observer too, after the policy has seen them.
    pub fn on_visible_range(
        &self,
        observer: impl FnMut(&ListScrollEvent, &mut Window, &mut App) + 'static,
    ) {
        self.inner.borrow_mut().visible_range = Some(Rc::new(RefCell::new(observer)));
    }

    // Queries.

    /// Whether the view is pinned to the bottom.
    pub fn is_pinned(&self) -> bool {
        self.inner.borrow().policy.is_pinned()
    }

    /// The distance from the viewport bottom to the content end, in px (0 at the end) (R2).
    pub fn distance_from_bottom(&self) -> f32 {
        let (_, max, scrolled) = self.geometry();
        (max - scrolled).max(0.0)
    }

    /// Whether the jump-to-latest affordance is shown (R5).
    pub fn show_jump_button(&self) -> bool {
        self.inner.borrow().policy.show_jump_button()
    }

    /// Whether an anchored send hold exists.
    pub fn has_hold(&self) -> bool {
        self.inner
            .borrow()
            .hold
            .is_some_and(|hold| hold.phase != HoldPhase::Released)
    }

    /// The same-frame reserved bottom height (R10).
    pub fn bottom_clearance(&self) -> f32 {
        self.inner.borrow().bottom_clearance
    }

    /// The right-side reservation (R10).
    pub fn right_reserve(&self) -> f32 {
        self.inner.borrow().right_reserve
    }

    /// Whether the follow stepper is idle (no frames needed) at the list's current position.
    pub fn is_idle(&self) -> bool {
        let inner = self.inner.borrow();
        !inner.kick
            && inner.hold.is_none()
            && inner.navigation.is_none()
            && (!inner.policy.is_pinned() || self.distance_from_bottom() <= LANDING_SNAP)
    }

    fn geometry(&self) -> (f32, f32, f32) {
        let viewport = f32::from(self.list.viewport_bounds().size.height);
        let max = f32::from(self.list.max_offset_for_scrollbar().y).max(0.0);
        let scrolled = (-f32::from(self.list.scroll_px_offset_for_scrollbar().y)).clamp(0.0, max);
        (viewport, max, scrolled)
    }

    fn is_glued(&self) -> bool {
        self.list.logical_scroll_top().item_ix >= self.list.item_count()
    }

    // List events.

    fn on_list_event(&self, event: &ListScrollEvent, window: &mut Window, cx: &mut App) {
        if event.user_initiated {
            self.on_user_scroll(window, cx);
        }
        if !event.user_initiated && self.inner.borrow().hold.is_some() {
            // The hold re-asserts its position after every layout that moved things.
            self.schedule(window);
        }
        let observer = self.inner.borrow().visible_range.clone();
        if let Some(observer) = observer {
            (observer.borrow_mut())(event, window, cx);
        }
    }

    /// The user-scroll handler (R2-R5, R13). Wheel and touch input only: programmatic scrolling
    /// never reaches it. A user scroll releases the hold and cancels the follow spring
    /// synchronously, before a queued frame can undo the input.
    pub fn on_user_scroll(&self, window: &mut Window, cx: &mut App) {
        let distance = self.distance_from_bottom();
        let change = {
            let mut inner = self.inner.borrow_mut();
            inner.stepper.reset();
            inner.navigation = None;
            inner.kick = false;
            inner.long_jump_done = false;
            if let Some(hold) = inner.hold.as_mut() {
                match hold.phase {
                    HoldPhase::Entering | HoldPhase::Holding => hold.phase = HoldPhase::Released,
                    HoldPhase::Released => {
                        // The hold re-arms only when the user returns to the hold position
                        // moving toward it (the item's top at or below inset - 2 px - 2 px).
                        if let Some(bounds) = self.list.bounds_for_item(hold.item) {
                            let top = f32::from(bounds.top() - self.list.viewport_bounds().top());
                            if top >= hold.inset - HOLD_SLACK - HOLD_TOLERANCE {
                                hold.phase = HoldPhase::Holding;
                            }
                        }
                    }
                }
            }
            inner.policy.on_user_scroll(distance)
        };
        if change == PinChange::Repinned {
            // A glide, not a jump.
            self.kick(window);
        }
        window.refresh();
        let _ = cx;
    }

    // Pin control.

    /// Pins and glides to the end (R4, R6).
    pub fn engage_pin(&self, window: &mut Window, cx: &mut App) {
        {
            let mut inner = self.inner.borrow_mut();
            inner.policy.set_pinned(true);
            inner.policy.hide_jump_button();
            inner.navigation = None;
        }
        if cx.reduce_motion() {
            self.land();
        } else {
            self.kick(window);
        }
        window.refresh();
    }

    /// Jump to latest (R5): re-pins. Reduced motion scrolls straight to the end; otherwise a
    /// distance beyond 2.5 viewports first teleports the excess, then the spring glides the rest.
    /// Jumping while an anchored send hold exists re-arms the hold instead of destroying the
    /// reservation, unless the anchored item is taller than the viewport.
    pub fn jump_to_bottom(&self, window: &mut Window, cx: &mut App) {
        let hold = self.inner.borrow().hold;
        if let Some(mut hold) = hold {
            let viewport = f32::from(self.list.viewport_bounds().size.height);
            let item_height = self
                .list
                .bounds_for_item(hold.item)
                .map(|bounds| f32::from(bounds.size.height))
                .unwrap_or(0.0);
            if item_height <= viewport {
                hold.phase = HoldPhase::Entering;
                let mut inner = self.inner.borrow_mut();
                inner.hold = Some(hold);
                inner.policy.hide_jump_button();
                drop(inner);
                self.kick(window);
                window.refresh();
                return;
            }
            // The anchored item is taller than the viewport: release the hold, engage the pin.
            self.release_hold();
        }
        {
            let mut inner = self.inner.borrow_mut();
            inner.policy.set_pinned(true);
            inner.policy.hide_jump_button();
            inner.navigation = None;
            inner.stepper.reset();
            inner.long_jump_done = false;
        }
        if cx.reduce_motion() {
            self.land();
        } else {
            let (viewport, _, _) = self.geometry();
            let excess = teleport_excess(self.distance_from_bottom(), viewport);
            if excess > 0.0 {
                self.list.scroll_by(px(excess));
            }
            self.inner.borrow_mut().long_jump_done = true;
            self.kick(window);
        }
        window.refresh();
    }

    /// Scrolls straight to the end anchor and records distance 0 (landing, R7).
    fn land(&self) {
        self.list.scroll_to_end();
        let mut inner = self.inner.borrow_mut();
        inner.policy.record_distance(0.0);
        inner.policy.update_jump_button(0.0);
        inner.kick = false;
    }

    /// Releases the anchored send hold (the reservation's runway stays) (R13).
    pub fn release_hold(&self) {
        self.inner.borrow_mut().hold = None;
    }

    // Explicit navigation (R15).

    /// Claims the viewport: discards holds, navigation and the spring, and unpins; the
    /// destination may re-pin at the end through R4 on the user's next input.
    pub fn begin_scroll_navigation(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.hold = None;
        inner.navigation = None;
        inner.stepper.reset();
        inner.kick = false;
        inner.long_jump_done = false;
        inner.policy.set_pinned(false);
    }

    /// Navigates to `row` so it sits at the viewport top (R21): scrolls immediately with reduced
    /// motion, otherwise glides on a fixed 500 ms ease-in-out timeline. Works for rows that were
    /// never measured.
    pub fn scroll_to_row(&self, row: usize, window: &mut Window, cx: &mut App) {
        self.begin_scroll_navigation();
        if cx.reduce_motion() {
            self.list.scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: px(0.0),
            });
            self.after_programmatic_move();
            window.refresh();
            return;
        }
        if self.is_glued() {
            let (viewport, _, _) = self.geometry();
            // Materialize the glued bottom as a true top anchor first.
            self.list.scroll_by(px(-(viewport + 0.5)));
        }
        let now = MotionClock::now_ms(cx);
        self.inner.borrow_mut().navigation = Some(Navigation {
            row,
            timeline: NavigationTimeline::new(now, NAVIGATION_DURATION_MS),
            items_ema: 0.0,
        });
        self.schedule(window);
        window.refresh();
    }

    fn after_programmatic_move(&self) {
        let distance = self.distance_from_bottom();
        let mut inner = self.inner.borrow_mut();
        inner.policy.record_distance(distance);
        inner.policy.update_jump_button(distance);
    }

    // Selection autoscroll (R14).

    /// Starts or ends a text-selection drag.
    pub fn set_selection_drag(&self, active: bool, window: &mut Window) {
        self.inner.borrow_mut().selection_drag = active;
        if !active {
            // Recompute the distance and the affordance; the pin is NOT re-engaged.
            self.after_programmatic_move();
            window.refresh();
        }
    }

    /// One autoscroll tick for a pointer at window `pointer_y` (R14): claims the viewport,
    /// scrolls programmatically by the quadratic ramp step and returns the step (zero outside the
    /// edge zones, where the ticker should stop).
    pub fn selection_autoscroll_tick(&self, pointer_y: f32, window: &mut Window) -> f32 {
        let bounds = self.list.viewport_bounds();
        let step = autoscroll_step(
            pointer_y,
            f32::from(bounds.top()),
            f32::from(bounds.bottom()),
        );
        if step != 0.0 {
            self.begin_scroll_navigation();
            self.list.scroll_by(px(step));
            self.after_programmatic_move();
            window.refresh();
        }
        step
    }

    // Reservations and chrome (R10).

    /// Sets the same-frame bottom chrome reservation: `clamp(requested, 0, limit)`. A change of
    /// more than 0.5 px wakes the stepper while pinned so the end rests above the new chrome, and
    /// re-measures the last row while anchored. Returns the reserved height; the list's viewport
    /// is the parent height minus it ([`list_viewport_height`]).
    pub fn set_bottom_clearance(&self, requested: f32, limit: f32, window: &mut Window) -> f32 {
        let reserved = reserved_height(requested, limit);
        let (changed, pinned, anchored) = {
            let mut inner = self.inner.borrow_mut();
            let changed = reservation_changed(inner.bottom_clearance, reserved);
            if changed {
                inner.bottom_clearance = reserved;
            }
            (changed, inner.policy.is_pinned(), inner.hold.is_some())
        };
        if changed {
            if pinned {
                self.kick(window);
            }
            if anchored {
                let count = self.list.item_count();
                if count > 0 {
                    self.list.remeasure_items(count - 1..count);
                }
            }
            window.refresh();
        }
        reserved
    }

    /// The list viewport height for a parent height (R10).
    pub fn list_height(&self, parent: f32) -> f32 {
        list_viewport_height(parent, self.bottom_clearance())
    }

    /// Sets a right-side reservation (a docked side card): the reading column shifts and all rows
    /// are re-measured.
    pub fn set_right_reserve(&self, reserve: f32, window: &mut Window) {
        let changed = {
            let mut inner = self.inner.borrow_mut();
            let changed = reservation_changed(inner.right_reserve, reserve);
            if changed {
                inner.right_reserve = reserve;
            }
            changed
        };
        if changed {
            self.list.remeasure();
            window.refresh();
        }
    }

    // Content commits (R8).

    /// Marks whether the tail row is streaming (a live-following commit needs it).
    pub fn set_tail_streaming(&self, streaming: bool) {
        self.inner.borrow_mut().tail_streaming = streaming;
    }

    /// Applies a content commit (R8). Before the commit it captures whether the view is
    /// live-following (pinned, tail streaming, at most 2 px from the end). Commits replacing rows
    /// in place re-measure them retaining their heights as hints, holding the scroll anchor;
    /// they never splice.
    pub fn commit(&self, commit: Commit, window: &mut Window, cx: &mut App) {
        let (pinned, live_following) = {
            let inner = self.inner.borrow();
            let live = inner.policy.is_pinned()
                && inner.tail_streaming
                && self.distance_from_bottom() <= AT_BOTTOM_EPSILON;
            (inner.policy.is_pinned(), live)
        };
        let was_empty = self.list.item_count() == 0;
        let glued = self.is_glued();
        match commit {
            Commit::Splice {
                old_range,
                new_count,
            } => self.list.splice(old_range, new_count),
            Commit::ReplaceInPlace(range) => self.list.remeasure_items(range),
        }
        if self.inner.borrow().hold.is_some() {
            self.schedule(window);
        }
        if !pinned {
            window.refresh();
            return;
        }
        if live_following {
            self.land();
            self.inner.borrow_mut().stepper.reset();
        } else if was_empty || cx.reduce_motion() {
            self.land();
        } else {
            if glued {
                // Glued would hard-snap to the new end on every commit: materialize a pixel
                // anchor just above the bottom so layout holds position and the stepper glides.
                let (viewport, _, _) = self.geometry();
                self.list.scroll_by(px(-(viewport + GLUE_ANCHOR)));
            }
            self.kick(window);
        }
        window.refresh();
    }

    // Anchored send (R12, R13).

    /// Starts an anchored turn for `item`: the list gets a trailing reservation so the item can
    /// rest at `inset` below the viewport top even when the following content is short, and an
    /// entry glide moves the view toward it.
    pub fn begin_anchored_send(&self, item: usize, inset: f32, window: &mut Window, cx: &mut App) {
        self.list.set_trailing_reservation(Some((item, px(inset))));
        {
            let mut inner = self.inner.borrow_mut();
            inner.policy.set_pinned(false);
            inner.policy.hide_jump_button();
            inner.stepper.reset();
            inner.navigation = None;
            inner.hold = Some(Hold {
                item,
                inset,
                phase: HoldPhase::Entering,
                last_ms: None,
            });
        }
        if cx.reduce_motion() {
            self.list.scroll_to(ListOffset {
                item_ix: item,
                offset_in_item: px(-inset),
            });
            if let Some(hold) = self.inner.borrow_mut().hold.as_mut() {
                hold.phase = HoldPhase::Holding;
            }
        } else {
            self.schedule(window);
        }
        window.refresh();
    }

    /// Leaving the conversation and returning clears the reservation (R13).
    pub fn clear_reservation(&self) {
        self.list.set_trailing_reservation(None);
        self.inner.borrow_mut().hold = None;
    }

    // Viewport save and restore (R16).

    /// Saves the reader's place: follow-tail when pinned, else the anchored viewport. `key_of`
    /// maps a row to its identity.
    pub fn save_viewport<K>(
        &self,
        key_of: impl Fn(usize) -> Option<K>,
    ) -> Option<ViewportSnapshot<K>> {
        if self.is_pinned() {
            return Some(ViewportSnapshot::FollowTail);
        }
        let top = self.list.logical_scroll_top();
        key_of(top.item_ix).map(|item| ViewportSnapshot::Anchored {
            item,
            offset: f32::from(top.offset_in_item),
            distance: self.distance_from_bottom(),
        })
    }

    /// Restores a snapshot: follow-tail conversations resume following; an anchored snapshot
    /// scrolls to the row `row_of` finds for the item.
    pub fn restore_viewport<K>(
        &self,
        snapshot: &ViewportSnapshot<K>,
        row_of: impl Fn(&K) -> Option<usize>,
        window: &mut Window,
        cx: &mut App,
    ) {
        match snapshot {
            ViewportSnapshot::FollowTail => {
                {
                    let mut inner = self.inner.borrow_mut();
                    inner.policy.set_pinned(true);
                }
                self.land();
                let _ = cx;
            }
            ViewportSnapshot::Anchored { item, offset, .. } => {
                if let Some(row) = row_of(item) {
                    self.begin_scroll_navigation();
                    self.list.scroll_to(ListOffset {
                        item_ix: row,
                        offset_in_item: px(*offset),
                    });
                    self.after_programmatic_move();
                }
            }
        }
        window.refresh();
    }

    // The per-frame stepper (R6, R7).

    fn kick(&self, window: &mut Window) {
        self.inner.borrow_mut().kick = true;
        self.schedule(window);
    }

    fn schedule(&self, window: &mut Window) {
        {
            let mut inner = self.inner.borrow_mut();
            if inner.frame_scheduled {
                return;
            }
            inner.frame_scheduled = true;
        }
        let controller = self.clone();
        window.on_next_frame(move |window, cx| {
            controller.inner.borrow_mut().frame_scheduled = false;
            if controller.tick(window, cx) {
                controller.schedule(window);
            }
            window.refresh();
        });
    }

    /// Runs one stepper tick after layout (so measurements are fresh); returns whether another
    /// frame is needed. A stationary spring never asks for one.
    pub fn tick(&self, window: &mut Window, cx: &mut App) -> bool {
        let now = MotionClock::now_ms(cx);
        let mut again = false;
        again |= self.tick_navigation(now, cx);
        again |= self.tick_hold(now, cx, window);
        again |= self.tick_follow(now, cx);
        again
    }

    fn tick_navigation(&self, now: f64, _cx: &mut App) -> bool {
        let Some(mut navigation) = self.inner.borrow_mut().navigation.take() else {
            return false;
        };
        let viewport = self.list.viewport_bounds();
        let finished = navigation.timeline.finished(now);
        let fraction = navigation.timeline.frame_fraction(now);
        if finished {
            self.list.scroll_to(ListOffset {
                item_ix: navigation.row,
                offset_in_item: px(0.0),
            });
            self.after_programmatic_move();
            return false;
        }
        match self.list.bounds_for_item(navigation.row) {
            Some(bounds) => {
                // Pixel-exact once the target row is measured.
                let distance = f32::from(bounds.top() - viewport.top());
                self.list.scroll_by(px(distance * fraction));
            }
            None => {
                // Item space: rows above (or below) are unmeasured.
                let top = self.list.logical_scroll_top();
                let top_height = (self.list.item_offset(top.item_ix + 1)
                    - self.list.item_offset(top.item_ix))
                .max(px(1.0));
                let here = top.item_ix as f32
                    + (f32::from(top.offset_in_item) / f32::from(top_height)).clamp(0.0, 1.0);
                let target = navigation.row as f32;
                let next = here - fraction * (here - target);
                let rows_visible = self.visible_rows().max(1) as f32;
                let estimate = f32::from(viewport.size.height) / rows_visible;
                navigation.items_ema = if navigation.items_ema == 0.0 {
                    estimate
                } else {
                    navigation.items_ema + 0.5 * (estimate - navigation.items_ema)
                };
                let step_px = (next - here) * navigation.items_ema;
                if step_px.abs() <= 0.8 * NAV_OVERDRAW {
                    self.list.scroll_by(px(step_px));
                } else {
                    let floor = next.floor().max(0.0);
                    self.list.scroll_to(ListOffset {
                        item_ix: floor as usize,
                        offset_in_item: px((next - floor) * navigation.items_ema),
                    });
                }
            }
        }
        self.inner.borrow_mut().navigation = Some(navigation);
        true
    }

    fn visible_rows(&self) -> usize {
        let viewport = self.list.viewport_bounds();
        let top = self.list.logical_scroll_top().item_ix;
        let mut count = 0;
        let mut ix = top;
        while let Some(bounds) = self.list.bounds_for_item(ix) {
            if bounds.top() >= viewport.bottom() || count >= 512 {
                break;
            }
            count += 1;
            ix += 1;
        }
        count
    }

    fn tick_hold(&self, now: f64, cx: &mut App, window: &mut Window) -> bool {
        let Some(mut hold) = self.inner.borrow().hold else {
            return false;
        };
        if hold.phase == HoldPhase::Released {
            return false;
        }
        // Handoff: the reservation is filled.
        if self.list.trailing_reservation_filled() {
            self.list.set_trailing_reservation(None);
            let (selection_drag, pinned) = {
                let inner = self.inner.borrow();
                (inner.selection_drag, inner.policy.is_pinned())
            };
            let engage = !selection_drag
                && (hold.phase != HoldPhase::Released
                    || pinned
                    || self.distance_from_bottom() <= AT_BOTTOM_EPSILON);
            self.inner.borrow_mut().hold = None;
            if engage {
                self.engage_pin(window, cx);
            }
            return false;
        }
        let viewport = self.list.viewport_bounds();
        let vp = f32::from(viewport.size.height);
        // The glide eases by `1 - 0.85^frames` per tick, frames in 60 fps units capped at 8.
        let frames = match hold.last_ms {
            Some(last) => ((now - last) / super::follow::FRAME_MS).clamp(0.0, 8.0) as f32,
            None => 1.0,
        };
        hold.last_ms = Some(now);
        let ease = 1.0 - HOLD_RETAIN.powf(frames);
        let error = self
            .list
            .bounds_for_item(hold.item)
            .map(|bounds| f32::from(bounds.top() - viewport.top()) - hold.inset);
        match (hold.phase, error) {
            (HoldPhase::Entering, Some(error)) => {
                let excess = error.abs() - teleport_excess_abs(vp);
                let mut error = error;
                if excess > 0.0 {
                    // Beyond 2.5 viewports the view first jumps the excess.
                    let jump = excess.copysign(error);
                    self.list.scroll_by(px(jump));
                    error -= jump;
                }
                if error.abs() <= 1.0 {
                    // The glide snaps within 1 px.
                    self.list.scroll_by(px(error));
                    hold.phase = HoldPhase::Holding;
                } else {
                    self.list.scroll_by(px(error * ease));
                }
                self.inner.borrow_mut().hold = Some(hold);
                true
            }
            (HoldPhase::Holding, Some(error)) => {
                // One-sided: only drift above the hold (the item lower than its inset) beyond
                // 0.5 px is corrected; resting inside the slack below is legal.
                self.inner.borrow_mut().hold = Some(hold);
                if error > HOLD_DRIFT {
                    self.list.scroll_by(px(error * ease));
                    return true;
                }
                false
            }
            (_, None) => {
                // Bounds unavailable (glued bottom or splice flicker): re-assert only when
                // farther than slack + 8 px from the end, by a direct scroll to the item.
                if self.distance_from_bottom() > HOLD_SLACK + 8.0 {
                    self.list.scroll_to(ListOffset {
                        item_ix: hold.item,
                        offset_in_item: px(-hold.inset),
                    });
                    hold.phase = HoldPhase::Holding;
                    self.inner.borrow_mut().hold = Some(hold);
                    true
                } else {
                    // Within reach of the end: treat as positioned.
                    hold.phase = HoldPhase::Holding;
                    self.inner.borrow_mut().hold = Some(hold);
                    false
                }
            }
            _ => false,
        }
    }

    fn tick_follow(&self, now: f64, cx: &mut App) -> bool {
        let (pinned, kick, hold) = {
            let inner = self.inner.borrow();
            (inner.policy.is_pinned(), inner.kick, inner.hold.is_some())
        };
        if !pinned || hold {
            self.inner.borrow_mut().kick = false;
            return false;
        }
        let (viewport, max, scrolled) = self.geometry();
        let distance = (max - scrolled).max(0.0);
        if distance <= LANDING_SNAP {
            // Landed: end anchor (not the estimated pixel total).
            if kick || self.inner.borrow().stepper.growth() != 0.0 || !self.is_glued() {
                self.land_keep_spring();
            }
            self.inner.borrow_mut().kick = false;
            return false;
        }
        if cx.reduce_motion() {
            self.land();
            return false;
        }
        let excess = teleport_excess(distance, viewport);
        let done = self.inner.borrow().long_jump_done;
        if excess > 0.0 && !done {
            self.list.scroll_by(px(excess));
            self.inner.borrow_mut().long_jump_done = true;
        }
        let (position, target) = {
            let (_, max, scrolled) = self.geometry();
            (scrolled, max)
        };
        let next = self.inner.borrow_mut().stepper.tick(now, position, target);
        if target - next <= LANDING_SNAP {
            self.land_keep_spring();
            self.inner.borrow_mut().long_jump_done = false;
            return false;
        }
        self.list.scroll_by(px(next - position));
        let distance = self.distance_from_bottom();
        let mut inner = self.inner.borrow_mut();
        inner.policy.record_distance(distance);
        inner.kick = false;
        true
    }

    /// Lands on the end anchor while keeping the spring state for the 500 ms grace (R7).
    fn land_keep_spring(&self) {
        self.list.scroll_to_end();
        let mut inner = self.inner.borrow_mut();
        inner.policy.record_distance(0.0);
        inner.kick = false;
    }
}

fn teleport_excess_abs(viewport: f32) -> f32 {
    super::follow::TELEPORT_VIEWPORTS * viewport
}
