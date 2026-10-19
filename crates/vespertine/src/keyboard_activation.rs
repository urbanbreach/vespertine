//! Keyboard activation of focused controls (INP-007 R4).
//!
//! A focused control activates on Enter (on key-down) and on Space (on
//! key-up). A Space press that another key interrupts, or during which focus
//! moved, does not activate. Auto-repeated key-downs never activate again: one
//! physical Enter press yields exactly one activation.
//!
//! The control owns a [`KeyboardActivation`] (it must persist across frames so
//! a key-down and the following key-up see the same state) and binds it to its
//! element with [`KeyboardActivation::bind`].

use std::{cell::RefCell, rc::Rc};

use crate::{App, InteractiveElement, KeyDownEvent, KeyUpEvent, Keystroke, Window};

/// Which key activated a control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationKey {
    /// Enter, activating on key-down.
    Enter,
    /// Space, activating on key-up.
    Space,
}

#[derive(Default)]
struct State {
    /// Set by the first Enter key-down, cleared by Enter's key-up.
    enter_latched: bool,
    /// Focus generation at which a Space key-down is waiting for its key-up.
    pending_space: Option<u64>,
}

/// Shared keyboard-activation state of one control.
#[derive(Clone, Default)]
pub struct KeyboardActivation {
    state: Rc<RefCell<State>>,
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

impl KeyboardActivation {
    /// New state with nothing pending.
    pub fn new() -> Self {
        Self::default()
    }

    /// Processes a key-down. Returns the activation it causes, which is only
    /// ever Enter. Activation keys' default is prevented so the base
    /// keyboard-click path cannot activate a second time on release.
    pub fn key_down(&self, event: &KeyDownEvent, window: &mut Window) -> Option<ActivationKey> {
        let mut state = self.state.borrow_mut();
        match activation_key(&event.keystroke) {
            Some(ActivationKey::Enter) => {
                window.prevent_default();
                if event.is_held || state.enter_latched {
                    return None;
                }
                state.enter_latched = true;
                Some(ActivationKey::Enter)
            }
            Some(ActivationKey::Space) => {
                window.prevent_default();
                if !event.is_held {
                    state.pending_space = Some(window.focus_generation);
                }
                None
            }
            None => {
                // Another key interrupts a pending Space press.
                state.pending_space = None;
                None
            }
        }
    }

    /// Processes a key-up. Returns the activation it causes, which is only
    /// ever Space, and only when its key-down was not interrupted and focus
    /// did not move in between.
    pub fn key_up(&self, event: &KeyUpEvent, window: &mut Window) -> Option<ActivationKey> {
        let mut state = self.state.borrow_mut();
        match activation_key(&event.keystroke) {
            Some(ActivationKey::Enter) => {
                window.prevent_default();
                state.enter_latched = false;
                None
            }
            Some(ActivationKey::Space) => {
                window.prevent_default();
                let pending = state.pending_space.take();
                (pending == Some(window.focus_generation)).then_some(ActivationKey::Space)
            }
            None => {
                state.pending_space = None;
                None
            }
        }
    }

    /// Binds this state to `element`: the element's key-down and key-up
    /// listeners call `on_activate` for each activation. The element must be
    /// focusable (track a focus handle) to receive key events.
    pub fn bind<E: InteractiveElement>(
        &self,
        element: E,
        on_activate: impl Fn(ActivationKey, &mut Window, &mut App) + 'static,
    ) -> E {
        let on_activate = Rc::new(on_activate);
        let down = self.clone();
        let up = self.clone();
        let on_down = on_activate.clone();
        element
            .on_key_down(move |event, window, cx| {
                if let Some(key) = down.key_down(event, window) {
                    on_down(key, window, cx);
                }
            })
            .on_key_up(move |event, window, cx| {
                if let Some(key) = up.key_up(event, window) {
                    on_activate(key, window, cx);
                }
            })
    }
}
