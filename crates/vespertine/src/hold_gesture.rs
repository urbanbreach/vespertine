//! Modifier-based hold gestures (INP-007 R6).
//!
//! A hold gesture is an action that runs while certain modifier keys are held
//! (for example "peek while Alt is down"). [`ModifierHold`] tracks one such
//! gesture for a focused surface and cancels it when any required modifier is
//! released, when the surface loses focus and when the window is deactivated.
//!
//! The surface feeds it modifier changes from its raw modifier listener
//! (`on_modifiers_changed`), which is delivered independently of any text
//! composition in progress.

use crate::{
    Context, EventEmitter, FocusHandle, Modifiers, ModifiersChangedEvent, Subscription, Window,
};

/// Why a hold gesture ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldEnd {
    /// A required modifier was released.
    ModifierReleased,
    /// The surface lost focus.
    FocusLost,
    /// The window was deactivated.
    WindowDeactivated,
}

/// Events of a [`ModifierHold`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierHoldEvent {
    /// All required modifiers are held while the surface is focused: the
    /// gesture's action starts.
    Started,
    /// The gesture ended and its action must stop.
    Ended(HoldEnd),
}

/// Tracks a hold gesture over a set of required modifiers for one surface.
pub struct ModifierHold {
    required: Modifiers,
    surface: FocusHandle,
    active: bool,
    _focus_out: Subscription,
    _window_activation: Subscription,
}

impl EventEmitter<ModifierHoldEvent> for ModifierHold {}

impl ModifierHold {
    /// A hold gesture requiring every modifier set in `required`, for the
    /// surface tracked by `surface`.
    pub fn new(
        required: Modifiers,
        surface: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let entity = cx.weak_entity();
        let focus_out = window.on_focus_out(surface, cx, move |_, _, cx| {
            entity
                .update(cx, |this, cx| this.end(HoldEnd::FocusLost, cx))
                .ok();
        });
        let window_activation = cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.end(HoldEnd::WindowDeactivated, cx);
            }
        });
        Self {
            required,
            surface: surface.clone(),
            active: false,
            _focus_out: focus_out,
            _window_activation: window_activation,
        }
    }

    /// Whether the gesture is currently running.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Feeds a modifier change from the surface's raw modifier listener.
    pub fn modifiers_changed(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        // A gesture that requires no modifier at all never runs.
        let held = self.required.modified() && holds_all(event.modifiers, self.required);
        if self.active && !held {
            self.end(HoldEnd::ModifierReleased, cx);
        } else if !self.active && held && self.surface.contains_focused(window, cx) {
            self.active = true;
            cx.emit(ModifierHoldEvent::Started);
        }
    }

    fn end(&mut self, end: HoldEnd, cx: &mut Context<Self>) {
        if self.active {
            self.active = false;
            cx.emit(ModifierHoldEvent::Ended(end));
        }
    }
}

fn holds_all(held: Modifiers, required: Modifiers) -> bool {
    (!required.control || held.control)
        && (!required.alt || held.alt)
        && (!required.shift || held.shift)
        && (!required.platform || held.platform)
        && (!required.function || held.function)
}
