//! The hover-detail state machine of chips (CMP-010 R8).
//!
//! The machine is driven by explicit events and timestamps; it owns no timer. [`HoverDetail`]
//! returns a [`TimerRequest`] when the owner must schedule a call to
//! [`HoverDetail::timer_fired`], and the generation counter makes stale timers harmless.

use super::reference::ReferenceKind;
use std::ops::Range;

/// How long the pointer must rest on a non-image chip before the detail popup shows.
pub const HOVER_DETAIL_DELAY_MS: u64 = 420;
/// Height of the detail popup in logical pixels.
pub const POPUP_HEIGHT: f32 = 24.0;
/// Gap between the popup and the chip when the popup is above it.
pub const POPUP_GAP: f32 = 1.0;

/// A chip as seen by the hit test: its source range and identity, and whether it is an image chip
/// (image chips never show a popup).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverChip {
    /// The chip's source range.
    pub range: Range<usize>,
    /// The chip's identity; together with the range it names the same chip across rebuilds.
    pub identity: String,
    /// Whether the chip is an image chip.
    pub image: bool,
}

impl HoverChip {
    fn target(&self) -> HoverTarget {
        HoverTarget {
            range: self.range.clone(),
            identity: self.identity.clone(),
        }
    }
}

/// The chip a hover wait or popup belongs to: the same source range and identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HoverTarget {
    /// The chip's source range.
    pub range: Range<usize>,
    /// The chip's identity.
    pub identity: String,
}

/// The lifecycle state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HoverState {
    /// No popup and no pending wait.
    Hidden,
    /// The pointer entered `target`; the popup shows once the timer of `generation` fires.
    Waiting {
        /// The chip being waited on.
        target: HoverTarget,
        /// The generation the timer must carry.
        generation: u64,
        /// The time the pointer entered the chip.
        since_ms: u64,
    },
    /// The popup for `target` is shown.
    Visible {
        /// The chip the popup describes.
        target: HoverTarget,
        /// The generation the popup was shown for.
        generation: u64,
    },
}

/// A request to call [`HoverDetail::timer_fired`] with `generation` at `fire_at_ms`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimerRequest {
    /// The generation to pass back.
    pub generation: u64,
    /// The time at which the timer must fire.
    pub fire_at_ms: u64,
}

/// The hover-detail state machine.
#[derive(Clone, Debug)]
pub struct HoverDetail {
    state: HoverState,
    generation: u64,
    popup_hovered: bool,
    pointer_on_target: bool,
}

impl Default for HoverDetail {
    fn default() -> Self {
        Self::new()
    }
}

impl HoverDetail {
    /// A hidden machine.
    pub fn new() -> Self {
        Self {
            state: HoverState::Hidden,
            generation: 0,
            popup_hovered: false,
            pointer_on_target: false,
        }
    }

    /// The current state.
    pub fn state(&self) -> &HoverState {
        &self.state
    }

    /// The current generation; every invalidation bumps it.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the popup is shown.
    pub fn is_visible(&self) -> bool {
        matches!(self.state, HoverState::Visible { .. })
    }

    /// The chip the popup is shown for, if visible.
    pub fn visible_target(&self) -> Option<&HoverTarget> {
        match &self.state {
            HoverState::Visible { target, .. } => Some(target),
            _ => None,
        }
    }

    fn current_target(&self) -> Option<&HoverTarget> {
        match &self.state {
            HoverState::Hidden => None,
            HoverState::Waiting { target, .. } | HoverState::Visible { target, .. } => Some(target),
        }
    }

    fn hide(&mut self) {
        self.generation += 1;
        self.state = HoverState::Hidden;
        self.popup_hovered = false;
        self.pointer_on_target = false;
    }

    /// The pointer is over `chip` (`None`: not over any chip) at `now_ms`. Entering a chip other
    /// than the current target starts a wait and returns the timer to schedule; motion inside the
    /// same chip changes nothing; leaving the chip hides the popup unless the pointer is in the
    /// popup while it is visible. Image chips never start a wait and count as no chip.
    pub fn pointer_over(&mut self, chip: Option<&HoverChip>, now_ms: u64) -> Option<TimerRequest> {
        let chip = chip.filter(|chip| !chip.image);
        match chip {
            Some(chip) => {
                let target = chip.target();
                if self.current_target() == Some(&target) {
                    self.pointer_on_target = true;
                    return None;
                }
                self.hide();
                self.pointer_on_target = true;
                self.state = HoverState::Waiting {
                    target,
                    generation: self.generation,
                    since_ms: now_ms,
                };
                Some(TimerRequest {
                    generation: self.generation,
                    fire_at_ms: now_ms + HOVER_DETAIL_DELAY_MS,
                })
            }
            None => {
                self.pointer_on_target = false;
                let keep = self.is_visible() && self.popup_hovered;
                if !keep && self.state != HoverState::Hidden {
                    self.hide();
                }
                None
            }
        }
    }

    /// The pointer entered the popup. Only meaningful while the popup is visible.
    pub fn popup_entered(&mut self) {
        if self.is_visible() {
            self.popup_hovered = true;
        }
    }

    /// The pointer left the popup: the popup hides unless the pointer is back over its chip.
    pub fn popup_left(&mut self) {
        self.popup_hovered = false;
        if self.is_visible() && !self.pointer_on_target {
            self.hide();
        }
    }

    /// The timer of `generation` fired at `now_ms`; `hit_list` holds the chips currently under the
    /// hit test. The state becomes visible only if the wait of that generation has run for the
    /// full delay and a chip with the same source range and identity still exists in the hit list;
    /// otherwise it is hidden. A timer of another generation, or one that fires early, is ignored.
    /// Returns whether the popup became visible.
    pub fn timer_fired(&mut self, generation: u64, now_ms: u64, hit_list: &[HoverChip]) -> bool {
        let HoverState::Waiting {
            target,
            generation: waiting,
            since_ms,
        } = &self.state
        else {
            return false;
        };
        if *waiting != generation || now_ms < since_ms + HOVER_DETAIL_DELAY_MS {
            return false;
        }
        let exists = hit_list.iter().any(|chip| {
            !chip.image && chip.range == target.range && chip.identity == target.identity
        });
        if exists {
            self.state = HoverState::Visible {
                target: target.clone(),
                generation,
            };
            true
        } else {
            self.hide();
            false
        }
    }

    /// Convenience for owners that poll: fires the pending timer if its time has come.
    pub fn tick(&mut self, now_ms: u64, hit_list: &[HoverChip]) -> bool {
        match self.state {
            HoverState::Waiting { generation, .. } => {
                self.timer_fired(generation, now_ms, hit_list)
            }
            _ => false,
        }
    }

    /// Any edit, scroll, wheel, mouse press, drag selection, undo/redo or value replacement:
    /// bumps the generation and hides.
    pub fn invalidate(&mut self) {
        self.hide();
    }

    /// The projection was rebuilt: `hit_list` is the new chip list. Invalidates when the target is
    /// no longer present (same source range and identity).
    pub fn projection_rebuilt(&mut self, hit_list: &[HoverChip]) {
        let Some(target) = self.current_target() else {
            return;
        };
        let present = hit_list
            .iter()
            .any(|chip| chip.range == target.range && chip.identity == target.identity);
        if !present {
            self.hide();
        }
    }
}

/// Which side of its chip the popup is placed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PopupSide {
    /// Above the chip with a 1 px gap.
    Above,
    /// Below the chip, flush with its bottom.
    Below,
}

/// The popup's top edge: above the chip with a 1 px gap, or below it flush with the chip bottom
/// when the space above (down to `available_top`) is too small for the 24 px popup.
pub fn popup_top(chip_top: f32, chip_bottom: f32, available_top: f32) -> (f32, PopupSide) {
    let above = chip_top - POPUP_GAP - POPUP_HEIGHT;
    if above >= available_top {
        (above, PopupSide::Above)
    } else {
        (chip_bottom, PopupSide::Below)
    }
}

/// The text the popup shows: the identity, with a trailing `/` for directories; attachments show
/// their name.
pub fn detail_text(kind: &ReferenceKind, name: &str, identity: &str, attachment: bool) -> String {
    if attachment || matches!(kind, ReferenceKind::Attachment | ReferenceKind::Image) {
        return name.to_owned();
    }
    if matches!(kind, ReferenceKind::Directory) && !identity.ends_with('/') {
        format!("{identity}/")
    } else {
        identity.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chip(range: Range<usize>, identity: &str) -> HoverChip {
        HoverChip {
            range,
            identity: identity.to_owned(),
            image: false,
        }
    }

    #[test]
    fn popup_shows_after_420_ms_of_rest_but_not_after_419() {
        let a = chip(0..10, "src/a.rs");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 1000).unwrap();
        assert_eq!(timer.fire_at_ms, 1420);
        assert!(matches!(hover.state(), HoverState::Waiting { .. }));
        assert!(!hover.timer_fired(timer.generation, 1419, std::slice::from_ref(&a)));
        assert!(!hover.is_visible());
        assert!(hover.timer_fired(timer.generation, 1420, std::slice::from_ref(&a)));
        assert!(hover.is_visible());
        assert_eq!(hover.visible_target().unwrap().identity, "src/a.rs");
    }

    #[test]
    fn tick_follows_the_clock() {
        let a = chip(0..10, "a");
        let mut hover = HoverDetail::new();
        hover.pointer_over(Some(&a), 0);
        assert!(!hover.tick(419, std::slice::from_ref(&a)));
        assert!(hover.tick(420, std::slice::from_ref(&a)));
        assert!(!hover.tick(500, &[a]));
    }

    #[test]
    fn moving_inside_the_chip_keeps_the_wait() {
        let a = chip(0..10, "a");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        assert!(hover.pointer_over(Some(&a), 200).is_none());
        assert!(hover.pointer_over(Some(&a), 400).is_none());
        assert_eq!(hover.generation(), timer.generation);
        assert!(hover.timer_fired(timer.generation, 420, &[a]));
    }

    #[test]
    fn moving_inside_a_visible_popup_chip_keeps_it_visible() {
        let a = chip(0..10, "a");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        hover.timer_fired(timer.generation, 420, std::slice::from_ref(&a));
        assert!(hover.pointer_over(Some(&a), 900).is_none());
        assert!(hover.is_visible());
    }

    #[test]
    fn moving_to_another_chip_restarts_the_wait() {
        let a = chip(0..10, "a");
        let b = chip(12..20, "b");
        let mut hover = HoverDetail::new();
        let first = hover.pointer_over(Some(&a), 0).unwrap();
        let second = hover.pointer_over(Some(&b), 300).unwrap();
        assert_ne!(first.generation, second.generation);
        assert_eq!(second.fire_at_ms, 720);
        // The first timer is stale: ignored, and it does not cancel the new wait.
        assert!(!hover.timer_fired(first.generation, 420, &[a.clone(), b.clone()]));
        assert!(matches!(hover.state(), HoverState::Waiting { .. }));
        assert!(!hover.timer_fired(second.generation, 719, &[a.clone(), b.clone()]));
        assert!(hover.timer_fired(second.generation, 720, &[a, b]));
        assert_eq!(hover.visible_target().unwrap().identity, "b");
    }

    #[test]
    fn moving_to_another_chip_while_visible_hides_and_waits_again() {
        let a = chip(0..10, "a");
        let b = chip(12..20, "b");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        hover.timer_fired(timer.generation, 420, std::slice::from_ref(&a));
        let next = hover.pointer_over(Some(&b), 500).unwrap();
        assert!(!hover.is_visible());
        assert!(matches!(hover.state(), HoverState::Waiting { .. }));
        assert_eq!(next.fire_at_ms, 920);
    }

    #[test]
    fn same_range_with_different_identity_is_another_chip() {
        let a = chip(0..10, "a");
        let renamed = chip(0..10, "other");
        let mut hover = HoverDetail::new();
        hover.pointer_over(Some(&a), 0);
        assert!(hover.pointer_over(Some(&renamed), 10).is_some());
    }

    #[test]
    fn leaving_the_chip_hides_and_cancels_the_wait() {
        let a = chip(0..10, "a");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        hover.pointer_over(None, 100);
        assert_eq!(hover.state(), &HoverState::Hidden);
        assert!(!hover.timer_fired(timer.generation, 420, &[a]));
        assert!(!hover.is_visible());
    }

    #[test]
    fn leaving_towards_the_popup_keeps_it_until_the_popup_is_left() {
        let a = chip(0..10, "a");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        hover.timer_fired(timer.generation, 420, std::slice::from_ref(&a));
        hover.popup_entered();
        hover.pointer_over(None, 500);
        assert!(hover.is_visible());
        hover.popup_left();
        assert!(!hover.is_visible());
    }

    #[test]
    fn leaving_the_popup_back_onto_the_chip_keeps_it() {
        let a = chip(0..10, "a");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        hover.timer_fired(timer.generation, 420, std::slice::from_ref(&a));
        hover.popup_entered();
        hover.pointer_over(None, 500);
        hover.pointer_over(Some(&a), 510);
        hover.popup_left();
        assert!(hover.is_visible());
    }

    #[test]
    fn popup_enter_while_not_visible_is_ignored() {
        let a = chip(0..10, "a");
        let mut hover = HoverDetail::new();
        hover.pointer_over(Some(&a), 0);
        hover.popup_entered();
        hover.pointer_over(None, 10);
        assert_eq!(hover.state(), &HoverState::Hidden);
    }

    #[test]
    fn invalidation_hides_and_makes_pending_timers_stale() {
        let a = chip(0..10, "a");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        let before = hover.generation();
        hover.invalidate();
        assert!(hover.generation() > before);
        assert_eq!(hover.state(), &HoverState::Hidden);
        assert!(!hover.timer_fired(timer.generation, 420, std::slice::from_ref(&a)));

        let timer = hover.pointer_over(Some(&a), 1000).unwrap();
        hover.timer_fired(timer.generation, 1420, &[a]);
        assert!(hover.is_visible());
        hover.invalidate();
        assert!(!hover.is_visible());
    }

    #[test]
    fn timer_hides_when_the_chip_is_gone_from_the_hit_list() {
        let a = chip(0..10, "a");
        let shifted = chip(3..13, "a");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        assert!(!hover.timer_fired(timer.generation, 420, &[shifted]));
        assert_eq!(hover.state(), &HoverState::Hidden);
    }

    #[test]
    fn projection_rebuild_removing_the_target_invalidates() {
        let a = chip(0..10, "a");
        let b = chip(12..20, "b");
        let mut hover = HoverDetail::new();
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        hover.timer_fired(timer.generation, 420, std::slice::from_ref(&a));
        hover.projection_rebuilt(&[a.clone(), b.clone()]);
        assert!(hover.is_visible());
        hover.projection_rebuilt(&[b]);
        assert!(!hover.is_visible());
    }

    #[test]
    fn image_chips_never_show_a_popup() {
        let image = HoverChip {
            range: 0..10,
            identity: "Image 1.png".into(),
            image: true,
        };
        let mut hover = HoverDetail::new();
        assert!(hover.pointer_over(Some(&image), 0).is_none());
        assert_eq!(hover.state(), &HoverState::Hidden);
        assert!(!hover.tick(1000, std::slice::from_ref(&image)));

        // Moving from a text chip onto an image chip hides the popup.
        let a = chip(12..20, "a");
        let timer = hover.pointer_over(Some(&a), 0).unwrap();
        hover.timer_fired(timer.generation, 420, &[a]);
        hover.pointer_over(Some(&image), 500);
        assert!(!hover.is_visible());
    }

    #[test]
    fn popup_is_above_with_a_gap_or_below_when_there_is_no_room() {
        assert_eq!(popup_top(100.0, 120.0, 0.0), (75.0, PopupSide::Above));
        assert_eq!(popup_top(25.0, 45.0, 0.0), (0.0, PopupSide::Above));
        assert_eq!(popup_top(24.0, 44.0, 0.0), (44.0, PopupSide::Below));
        assert_eq!(popup_top(100.0, 120.0, 90.0), (120.0, PopupSide::Below));
    }

    #[test]
    fn detail_text_shows_identity_directory_slash_or_attachment_name() {
        assert_eq!(
            detail_text(&ReferenceKind::File, "a.rs", "src/a.rs", false),
            "src/a.rs"
        );
        assert_eq!(
            detail_text(&ReferenceKind::Directory, "src", "/p/src", false),
            "/p/src/"
        );
        assert_eq!(
            detail_text(&ReferenceKind::Directory, "src", "/p/src/", false),
            "/p/src/"
        );
        assert_eq!(
            detail_text(&ReferenceKind::Attachment, "notes.pdf", "/tmp/x", true),
            "notes.pdf"
        );
        assert_eq!(
            detail_text(&ReferenceKind::File, "Image 1.png", "/tmp/x", true),
            "Image 1.png"
        );
    }
}
