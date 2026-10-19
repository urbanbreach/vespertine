//! Activation rules of button-like controls (CMP-001 R2).
//!
//! One physical key press activates a control exactly once:
//!
//! * **Pointer**: a primary-button click on an enabled control activates it; the control's
//!   click listener does that, this module only covers the keyboard.
//! * **Enter** activates on the first non-held key-down. The key-down default is consumed (and
//!   so is the matching key-up default) so the base keyboard-click path, which fires on key
//!   release, cannot activate a second time.
//! * **Space** has its key-down default consumed and activates on key-up, provided no other
//!   key event intervened and the control did not lose focus in between.
//! * **Repeats**: X11 can deliver repeated key-downs that are not flagged as held. The latch
//!   therefore remembers that Enter is down (set by the first Enter key-down, cleared by Enter's
//!   key-up) and ignores every further Enter key-down until the physical key is released, in
//!   addition to ignoring key-downs flagged as held.
//! * A disabled control, or a key event whose default was already prevented (for example by a
//!   caller's key handler, which runs before the internal one) never activates.
//!
//! A control that registers only a click listener and no latch keeps the base keyboard-click
//! behavior unchanged: Enter or Space without modifiers activates once, on key release, if no
//! other key event intervened and focus did not move.
//!
//! [`ActivationLatch`] holds the per-control state. It is cheap to clone (clones share the
//! state), must persist across frames, and is bound to an element with
//! [`ActivationLatch::bind`]:
//!
//! ```ignore
//! let latch = ActivationLatch::new();
//! let button = latch.bind(div().id("go").track_focus(&focus), enabled, |key, window, cx| {
//!     // exactly one call per physical press
//! });
//! ```

use std::{cell::RefCell, rc::Rc};

use vespertine::{
    App, Context, FocusHandle, InteractiveElement, KeyDownEvent, KeyUpEvent, Keystroke,
    Subscription, Window,
};

/// Which key activated a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationKey {
    /// Enter, which activates on key-down.
    Enter,
    /// Space, which activates on key-up.
    Space,
}

/// What a key event means to the latch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyOutcome {
    /// The activation this event causes, if any.
    pub activate: Option<ActivationKey>,
    /// Whether the event's default must be consumed (prevented), so the platform keyboard-click
    /// path cannot activate a second time.
    pub consume: bool,
}

#[derive(Default)]
struct LatchState {
    /// Set by the first Enter key-down, cleared by Enter's key-up.
    enter_down: bool,
    /// A Space key-down is waiting for its key-up.
    space_down: bool,
}

/// The per-control activation latch.
#[derive(Clone, Default)]
pub struct ActivationLatch {
    state: Rc<RefCell<LatchState>>,
}

fn activation_key(keystroke: &Keystroke) -> Option<ActivationKey> {
    if keystroke.modifiers.modified() {
        return None;
    }
    match keystroke.key.as_str() {
        "enter" => Some(ActivationKey::Enter),
        "space" => Some(ActivationKey::Space),
        _ => None,
    }
}

impl ActivationLatch {
    /// A latch with no key down.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the latch currently considers a key to be down (Enter latched or a Space press
    /// pending).
    pub fn is_down(&self) -> bool {
        let state = self.state.borrow();
        state.enter_down || state.space_down
    }

    /// Forgets any key that is down. Call it when the control loses focus: the matching key-up
    /// then goes elsewhere and would never clear the latch. [`ActivationLatch::watch_blur`] does
    /// it for a focus handle.
    pub fn reset(&self) {
        *self.state.borrow_mut() = LatchState::default();
    }

    /// Resets this latch whenever `focus` loses focus. Keep the returned subscription alive as
    /// long as the control exists.
    pub fn watch_blur<T: 'static>(
        &self,
        focus: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Subscription {
        let latch = self.clone();
        cx.on_blur(focus, window, move |_, _, _| latch.reset())
    }

    /// Processes a key-down of the control, independent of any window: `is_held` is the event's
    /// held flag. Unmodified Enter activates on its first key-down only; unmodified Space only
    /// records that it is down; every other key interrupts a pending Space press.
    pub fn press(&self, keystroke: &Keystroke, is_held: bool) -> KeyOutcome {
        let mut state = self.state.borrow_mut();
        match activation_key(keystroke) {
            Some(ActivationKey::Enter) => {
                let activate = if is_held || state.enter_down {
                    None
                } else {
                    state.enter_down = true;
                    Some(ActivationKey::Enter)
                };
                KeyOutcome {
                    activate,
                    consume: true,
                }
            }
            Some(ActivationKey::Space) => {
                if !is_held {
                    state.space_down = true;
                }
                KeyOutcome {
                    activate: None,
                    consume: true,
                }
            }
            None => {
                state.space_down = false;
                KeyOutcome::default()
            }
        }
    }

    /// Processes a key-up of the control, independent of any window. Enter's key-up clears the
    /// latch. Space's key-up activates if its key-down was not interrupted.
    pub fn release(&self, keystroke: &Keystroke) -> KeyOutcome {
        let mut state = self.state.borrow_mut();
        match activation_key(keystroke) {
            Some(ActivationKey::Enter) => KeyOutcome {
                activate: None,
                consume: std::mem::take(&mut state.enter_down),
            },
            Some(ActivationKey::Space) => {
                let was_down = std::mem::take(&mut state.space_down);
                KeyOutcome {
                    activate: was_down.then_some(ActivationKey::Space),
                    consume: was_down,
                }
            }
            None => {
                state.space_down = false;
                KeyOutcome::default()
            }
        }
    }

    /// Handles a key-down event of an element: returns the activation, which is only ever
    /// Enter, and consumes the default of activation keys. A disabled control and an event
    /// whose default was already prevented do nothing.
    pub fn on_key_down(
        &self,
        event: &KeyDownEvent,
        enabled: bool,
        window: &mut Window,
    ) -> Option<ActivationKey> {
        if !enabled || window.default_prevented() {
            return None;
        }
        let outcome = self.press(&event.keystroke, event.is_held);
        if outcome.consume {
            window.prevent_default();
        }
        outcome.activate
    }

    /// Handles a key-up event of an element: returns the activation, which is only ever Space.
    pub fn on_key_up(
        &self,
        event: &KeyUpEvent,
        enabled: bool,
        window: &mut Window,
    ) -> Option<ActivationKey> {
        if !enabled {
            self.reset();
            return None;
        }
        if window.default_prevented() {
            return None;
        }
        let outcome = self.release(&event.keystroke);
        if outcome.consume {
            window.prevent_default();
        }
        outcome.activate
    }

    /// Binds this latch to `element`: its key-down and key-up listeners call `on_activate`
    /// once per physical press. Listeners the caller registered on the element before this
    /// call run first (they may prevent the default and thereby veto the activation). The
    /// element must track a focus handle to receive key events. With `enabled` false nothing
    /// activates.
    pub fn bind<E: InteractiveElement>(
        &self,
        element: E,
        enabled: bool,
        on_activate: impl Fn(ActivationKey, &mut Window, &mut App) + 'static,
    ) -> E {
        let on_activate = Rc::new(on_activate);
        let down = self.clone();
        let up = self.clone();
        let on_down = on_activate.clone();
        element
            .on_key_down(move |event, window, cx| {
                if let Some(key) = down.on_key_down(event, enabled, window) {
                    on_down(key, window, cx);
                }
            })
            .on_key_up(move |event, window, cx| {
                if let Some(key) = up.on_key_up(event, enabled, window) {
                    on_activate(key, window, cx);
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Keystroke {
        Keystroke::parse(name).unwrap()
    }

    #[test]
    fn enter_activates_once_per_physical_press_including_unflagged_repeats() {
        let latch = ActivationLatch::new();
        let enter = key("enter");
        assert_eq!(
            latch.press(&enter, false).activate,
            Some(ActivationKey::Enter)
        );
        // A repeat flagged as held and one that is not flagged are both ignored, as many times
        // as the platform delivers them.
        for held in [true, false, true, false, false] {
            let outcome = latch.press(&enter, held);
            assert_eq!(outcome.activate, None, "repeat held={held}");
            assert!(outcome.consume, "the default is still consumed");
        }
        assert!(latch.is_down());
        // The key-up clears the latch and consumes the default so the base keyboard-click path
        // cannot fire on release.
        let released = latch.release(&enter);
        assert_eq!(released.activate, None);
        assert!(released.consume);
        assert!(!latch.is_down());
        // The next physical press activates again.
        assert_eq!(
            latch.press(&enter, false).activate,
            Some(ActivationKey::Enter)
        );
    }

    #[test]
    fn a_key_down_flagged_as_held_never_starts_an_activation() {
        let latch = ActivationLatch::new();
        assert_eq!(latch.press(&key("enter"), true).activate, None);
        assert!(!latch.is_down(), "a held first key-down does not latch");
        assert_eq!(
            latch.press(&key("enter"), false).activate,
            Some(ActivationKey::Enter)
        );
    }

    #[test]
    fn space_activates_on_release_only() {
        let latch = ActivationLatch::new();
        let space = key("space");
        let down = latch.press(&space, false);
        assert_eq!(down.activate, None);
        assert!(down.consume, "the key-down default is prevented");
        assert_eq!(latch.press(&space, true).activate, None);
        assert_eq!(
            latch.press(&space, false).activate,
            None,
            "unflagged repeat"
        );
        let up = latch.release(&space);
        assert_eq!(up.activate, Some(ActivationKey::Space));
        assert!(up.consume);
        // A second release without a press does nothing.
        assert_eq!(latch.release(&space).activate, None);
    }

    #[test]
    fn another_key_interrupts_a_pending_space_press() {
        let latch = ActivationLatch::new();
        latch.press(&key("space"), false);
        latch.press(&key("a"), false);
        assert_eq!(latch.release(&key("space")).activate, None);

        latch.press(&key("space"), false);
        latch.release(&key("a"));
        assert_eq!(latch.release(&key("space")).activate, None);
    }

    #[test]
    fn modified_keys_never_activate() {
        let latch = ActivationLatch::new();
        for stroke in ["shift-enter", "ctrl-enter", "alt-space", "cmd-space"] {
            let outcome = latch.press(&key(stroke), false);
            assert_eq!(outcome, KeyOutcome::default(), "{stroke}");
            assert_eq!(
                latch.release(&key(stroke)),
                KeyOutcome::default(),
                "{stroke}"
            );
        }
    }

    #[test]
    fn reset_forgets_a_key_that_was_released_elsewhere() {
        let latch = ActivationLatch::new();
        latch.press(&key("enter"), false);
        latch.reset();
        assert_eq!(
            latch.press(&key("enter"), false).activate,
            Some(ActivationKey::Enter)
        );
        latch.reset();
        latch.press(&key("space"), false);
        latch.reset();
        assert_eq!(latch.release(&key("space")).activate, None);
    }

    #[test]
    fn enter_release_that_never_pressed_here_is_not_consumed() {
        let latch = ActivationLatch::new();
        assert_eq!(latch.release(&key("enter")), KeyOutcome::default());
    }
}
