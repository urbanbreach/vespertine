//! Hover-card timing: dwell, warm window, cold dismissal and the interactive hide grace
//! (CMP-004 R12-R14).
//!
//! Both state machines are pure: the owner passes a monotonic `now` and starts the timers they
//! ask for, so every rule is testable with a fake clock.

use crate::floating::hover_intent::{RENEW_THRESHOLD, TURN_AWAY_THRESHOLD, in_corridor};
use std::time::Duration;
use vespertine::{Bounds, Pixels, Point};

/// Pointer rest time before a cold card opens.
pub const DWELL: Duration = Duration::from_millis(550);
/// A card that hid at most this long ago lets the next one open at once (inclusive).
pub const WARM_WINDOW: Duration = Duration::from_millis(400);
/// Hide grace of an interactive card.
pub const INTERACTIVE_HIDE_GRACE: Duration = Duration::from_millis(80);

/// What the owner does after an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reaction<K> {
    /// Nothing changes.
    None,
    /// Show the card of this trigger now.
    Show(K),
    /// Start the dwell timer; when it fires call [`HoverCards::settle`].
    StartDwell {
        /// The trigger being dwelt on.
        target: K,
        /// The monotonic time at which the dwell settles.
        due: Duration,
    },
    /// Hide the shown card.
    Hide(K),
}

/// The controller shared by every hover-card trigger of a container: shown target, pending
/// target with its dwell, and the hidden-at time.
#[derive(Clone, Debug)]
pub struct HoverCards<K> {
    shown: Option<K>,
    pending: Option<(K, Duration)>,
    hidden_at: Option<Duration>,
}

impl<K> Default for HoverCards<K> {
    fn default() -> Self {
        Self {
            shown: None,
            pending: None,
            hidden_at: None,
        }
    }
}

impl<K: Clone + PartialEq> HoverCards<K> {
    /// A controller with nothing shown.
    pub fn new() -> Self {
        Self::default()
    }

    /// The trigger whose card is shown.
    pub fn shown(&self) -> Option<&K> {
        self.shown.as_ref()
    }

    /// The trigger with a pending dwell.
    pub fn target(&self) -> Option<&K> {
        self.pending.as_ref().map(|(key, _)| key)
    }

    /// When the pending dwell settles.
    pub fn due(&self) -> Option<Duration> {
        self.pending.as_ref().map(|(_, due)| *due)
    }

    /// The pointer entered `key`'s trigger. `blocked` is true while a drag is active or a menu of
    /// the same group is open: no card starts then (R12).
    pub fn enter(&mut self, now: Duration, key: &K, blocked: bool) -> Reaction<K> {
        if self.shown.as_ref() == Some(key) {
            return Reaction::None;
        }
        if blocked {
            self.pending = None;
            return Reaction::None;
        }
        let warm = self.shown.is_some()
            || self
                .hidden_at
                .is_some_and(|hidden| now.saturating_sub(hidden) <= WARM_WINDOW);
        if warm {
            self.pending = None;
            self.shown = Some(key.clone());
            return Reaction::Show(key.clone());
        }
        if self.pending.as_ref().map(|(pending, _)| pending) == Some(key) {
            return Reaction::None;
        }
        let due = now + DWELL;
        self.pending = Some((key.clone(), due));
        Reaction::StartDwell {
            target: key.clone(),
            due,
        }
    }

    /// The dwell timer of `key` fired: the card shows when the pending target is still `key`
    /// (leaving cancels it) and the dwell has elapsed.
    pub fn settle(&mut self, now: Duration, key: &K) -> Reaction<K> {
        match &self.pending {
            Some((pending, due)) if pending == key && *due <= now => {
                self.pending = None;
                self.shown = Some(key.clone());
                Reaction::Show(key.clone())
            }
            _ => Reaction::None,
        }
    }

    /// The pointer left `key`'s trigger: cancels its pending dwell; when its card is shown it
    /// hides and the hidden-at time is recorded.
    pub fn leave(&mut self, now: Duration, key: &K) -> Reaction<K> {
        if self.pending.as_ref().map(|(pending, _)| pending) == Some(key) {
            self.pending = None;
        }
        if self.shown.as_ref() == Some(key) {
            self.shown = None;
            self.hidden_at = Some(now);
            return Reaction::Hide(key.clone());
        }
        Reaction::None
    }

    /// A click, menu opening, drag start or scroll: cancels pending, hides the shown card and
    /// clears hidden-at so the card does not chase the pointer straight back.
    pub fn dismiss_cold(&mut self) -> Reaction<K> {
        self.pending = None;
        self.hidden_at = None;
        match self.shown.take() {
            Some(key) => Reaction::Hide(key),
            None => Reaction::None,
        }
    }

    /// Render-time reconciliation (R13): a card whose trigger is no longer mounted or visible, is
    /// being edited, or while a drag is active, is dismissed cold; so is a pending dwell whose
    /// trigger went away.
    pub fn reconcile(
        &mut self,
        trigger_visible: impl Fn(&K) -> bool,
        editing: bool,
        drag_active: bool,
    ) -> Reaction<K> {
        if let Some((key, _)) = &self.pending {
            if !trigger_visible(key) || drag_active {
                self.pending = None;
            }
        }
        let dismiss = self
            .shown
            .as_ref()
            .is_some_and(|key| !trigger_visible(key) || editing || drag_active);
        if dismiss {
            self.dismiss_cold()
        } else {
            Reaction::None
        }
    }
}

/// What the owner does with an interactive card's hide timer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideAnswer {
    /// Nothing; a running grace timer continues.
    None,
    /// (Re)start the grace timer; hide when it fires unless cancelled.
    StartGrace,
    /// Hide now (the pointer turned away).
    Close,
    /// A pending grace timer is cancelled (the pointer is on the card).
    CancelGrace,
}

/// Hide policy of a card with interactive content (R14): leaving the trigger starts the 80 ms
/// grace unless the pointer is inside the corridor toward the card; while the pointer travels the
/// corridor a forward move of at least 2 px renews, a move of −2 px or more away closes, and
/// leaving the corridor starts the grace. Entering the card cancels the pending close; leaving
/// the card starts it again.
#[derive(Clone, Debug, Default)]
pub struct InteractiveHide {
    origin: Option<Point<Pixels>>,
    last: Option<Point<Pixels>>,
    grace_due: Option<Duration>,
    tracking: bool,
}

impl InteractiveHide {
    /// A policy with no origin.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the pointer while it is on the trigger (the corridor's origin).
    pub fn record_origin(&mut self, pointer: Point<Pixels>) {
        self.origin = Some(pointer);
    }

    /// When the grace expires, if running.
    pub fn grace_due(&self) -> Option<Duration> {
        self.grace_due
    }

    fn start_grace(&mut self, now: Duration) -> HideAnswer {
        self.grace_due = Some(now + INTERACTIVE_HIDE_GRACE);
        HideAnswer::StartGrace
    }

    /// The pointer left the trigger at `pointer`. The card's `bounds` are the corridor's child;
    /// `left` is true for a card opening to the left of the trigger.
    pub fn trigger_left(
        &mut self,
        now: Duration,
        pointer: Point<Pixels>,
        card: Bounds<Pixels>,
        left: bool,
    ) -> HideAnswer {
        let origin = self.origin.unwrap_or(pointer);
        self.last = Some(pointer);
        self.tracking = in_corridor(origin, pointer, card, left);
        // The grace runs from the leave: forward progress renews it, a pause lets it expire.
        self.start_grace(now)
    }

    /// The pointer moved while the card is open and it is not on the trigger or the card.
    pub fn moved(
        &mut self,
        now: Duration,
        pointer: Point<Pixels>,
        card: Bounds<Pixels>,
        left: bool,
    ) -> HideAnswer {
        if !self.tracking {
            return HideAnswer::None;
        }
        let origin = self.origin.unwrap_or(pointer);
        if !in_corridor(origin, pointer, card, left) {
            // Leaving the corridor stops tracking; the running grace expires on its own.
            self.tracking = false;
            return HideAnswer::None;
        }
        let direction = if left { -1.0 } else { 1.0 };
        let forward = self
            .last
            .map_or(0.0, |last| f32::from(pointer.x - last.x) * direction);
        if forward <= TURN_AWAY_THRESHOLD {
            self.tracking = false;
            self.grace_due = None;
            return HideAnswer::Close;
        }
        if forward >= RENEW_THRESHOLD {
            self.last = Some(pointer);
            return self.start_grace(now);
        }
        HideAnswer::None
    }

    /// The pointer entered the card: the pending close is cancelled.
    pub fn card_entered(&mut self) -> HideAnswer {
        self.tracking = false;
        if self.grace_due.take().is_some() {
            HideAnswer::CancelGrace
        } else {
            HideAnswer::None
        }
    }

    /// The pointer left the card: the grace starts again.
    pub fn card_left(&mut self, now: Duration) -> HideAnswer {
        self.tracking = false;
        self.start_grace(now)
    }

    /// Whether the grace has expired at `now` (the card hides); consumes it.
    pub fn expired(&mut self, now: Duration) -> bool {
        match self.grace_due {
            Some(due) if due <= now => {
                self.grace_due = None;
                true
            }
            _ => false,
        }
    }

    /// The trigger was re-entered: the pending close is cancelled.
    pub fn trigger_entered(&mut self) -> HideAnswer {
        self.tracking = false;
        if self.grace_due.take().is_some() {
            HideAnswer::CancelGrace
        } else {
            HideAnswer::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vespertine::{point, px, size};

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn the_first_card_shows_after_the_dwell_with_the_pointer_still_on_the_trigger() {
        let mut cards = HoverCards::<u32>::new();
        let reaction = cards.enter(ms(0), &1, false);
        assert_eq!(
            reaction,
            Reaction::StartDwell {
                target: 1,
                due: ms(550)
            }
        );
        assert_eq!(cards.settle(ms(549), &1), Reaction::None);
        assert_eq!(cards.settle(ms(550), &1), Reaction::Show(1));
        assert_eq!(cards.shown(), Some(&1));
    }

    #[test]
    fn leaving_cancels_a_pending_dwell() {
        let mut cards = HoverCards::<u32>::new();
        cards.enter(ms(0), &1, false);
        assert_eq!(cards.leave(ms(100), &1), Reaction::None);
        assert_eq!(cards.settle(ms(600), &1), Reaction::None);
        assert_eq!(cards.shown(), None);
    }

    #[test]
    fn moving_to_a_neighbor_while_shown_shows_at_once() {
        let mut cards = HoverCards::<u32>::new();
        cards.enter(ms(0), &1, false);
        cards.settle(ms(550), &1);
        assert_eq!(cards.enter(ms(600), &2, false), Reaction::Show(2));
        assert_eq!(
            cards.enter(ms(610), &2, false),
            Reaction::None,
            "already shown"
        );
    }

    #[test]
    fn the_warm_window_is_inclusive_at_400_ms() {
        let mut cards = HoverCards::<u32>::new();
        cards.enter(ms(0), &1, false);
        cards.settle(ms(550), &1);
        assert_eq!(cards.leave(ms(1000), &1), Reaction::Hide(1));
        assert_eq!(cards.enter(ms(1400), &2, false), Reaction::Show(2));
        cards.leave(ms(1500), &2);
        assert_eq!(
            cards.enter(ms(1901), &1, false),
            Reaction::StartDwell {
                target: 1,
                due: ms(2451)
            },
            "401 ms after the hide waits the dwell"
        );
    }

    #[test]
    fn a_cold_dismissal_clears_the_warm_window_and_pending() {
        let mut cards = HoverCards::<u32>::new();
        cards.enter(ms(0), &1, false);
        cards.settle(ms(550), &1);
        assert_eq!(cards.dismiss_cold(), Reaction::Hide(1));
        assert_eq!(
            cards.enter(ms(600), &1, false),
            Reaction::StartDwell {
                target: 1,
                due: ms(1150)
            },
            "a re-enter right after a click waits the dwell"
        );
        assert_eq!(cards.dismiss_cold(), Reaction::None);
        assert_eq!(cards.target(), None);
    }

    #[test]
    fn no_card_starts_while_blocked() {
        let mut cards = HoverCards::<u32>::new();
        assert_eq!(cards.enter(ms(0), &1, true), Reaction::None);
        assert_eq!(cards.target(), None);
    }

    #[test]
    fn reconcile_dismisses_unmounted_edited_or_dragged_cards_and_cancels_stale_dwells() {
        let mut cards = HoverCards::<u32>::new();
        cards.enter(ms(0), &1, false);
        cards.settle(ms(550), &1);
        assert_eq!(cards.reconcile(|_| true, false, false), Reaction::None);
        assert_eq!(cards.reconcile(|_| false, false, false), Reaction::Hide(1));

        cards.enter(ms(1000), &2, false);
        cards.settle(ms(1550), &2);
        assert_eq!(cards.reconcile(|_| true, true, false), Reaction::Hide(2));
        cards.enter(ms(2000), &3, false);
        cards.settle(ms(2550), &3);
        assert_eq!(cards.reconcile(|_| true, false, true), Reaction::Hide(3));

        // A clipped trigger cancels the pending dwell before it settles.
        cards.enter(ms(5000), &4, false);
        assert_eq!(cards.reconcile(|_| false, false, false), Reaction::None);
        assert_eq!(cards.settle(ms(6000), &4), Reaction::None);
    }

    fn card() -> Bounds<Pixels> {
        Bounds::new(point(px(200.), px(80.)), size(px(80.), px(160.)))
    }

    #[test]
    fn forward_travel_through_the_corridor_renews_and_a_reversal_closes() {
        let mut hide = InteractiveHide::new();
        hide.record_origin(point(px(100.), px(100.)));
        let answer = hide.trigger_left(ms(0), point(px(110.), px(105.)), card(), false);
        assert_eq!(answer, HideAnswer::StartGrace);
        // Forward moves at least 2 px apart keep renewing the grace.
        assert_eq!(
            hide.moved(ms(60), point(px(115.), px(108.)), card(), false),
            HideAnswer::StartGrace
        );
        assert_eq!(hide.grace_due(), Some(ms(140)));
        assert_eq!(
            hide.moved(ms(100), point(px(120.), px(110.)), card(), false),
            HideAnswer::StartGrace
        );
        assert!(!hide.expired(ms(179)));
        // A small movement keeps the running timer.
        assert_eq!(
            hide.moved(ms(150), point(px(120.5), px(110.)), card(), false),
            HideAnswer::None
        );
        // A reversal of 2 px or more closes at once.
        assert_eq!(
            hide.moved(ms(160), point(px(117.), px(110.)), card(), false),
            HideAnswer::Close
        );
        assert_eq!(hide.grace_due(), None);
    }

    #[test]
    fn leaving_the_corridor_lets_the_grace_expire_after_80_ms() {
        let mut hide = InteractiveHide::new();
        hide.record_origin(point(px(100.), px(100.)));
        hide.trigger_left(ms(0), point(px(110.), px(105.)), card(), false);
        // Far above the funnel: out of the corridor, no renewal.
        assert_eq!(
            hide.moved(ms(40), point(px(130.), px(0.)), card(), false),
            HideAnswer::None
        );
        assert_eq!(
            hide.moved(ms(50), point(px(140.), px(0.)), card(), false),
            HideAnswer::None
        );
        assert!(!hide.expired(ms(79)));
        assert!(hide.expired(ms(80)));
    }

    #[test]
    fn entering_the_card_cancels_and_leaving_it_starts_the_grace_again() {
        let mut hide = InteractiveHide::new();
        hide.trigger_left(ms(0), point(px(110.), px(300.)), card(), false);
        assert_eq!(hide.card_entered(), HideAnswer::CancelGrace);
        assert!(!hide.expired(ms(500)));
        assert_eq!(hide.card_left(ms(600)), HideAnswer::StartGrace);
        assert!(hide.expired(ms(680)));
        assert_eq!(hide.trigger_entered(), HideAnswer::None);
    }

    #[test]
    fn the_mirrored_left_opening_card_behaves_identically() {
        let card = Bounds::new(point(px(20.), px(80.)), size(px(80.), px(160.)));
        let mut hide = InteractiveHide::new();
        hide.record_origin(point(px(200.), px(100.)));
        hide.trigger_left(ms(0), point(px(190.), px(105.)), card, true);
        assert_eq!(
            hide.moved(ms(60), point(px(185.), px(108.)), card, true),
            HideAnswer::StartGrace
        );
        assert_eq!(
            hide.moved(ms(100), point(px(188.), px(108.)), card, true),
            HideAnswer::Close
        );
    }
}
