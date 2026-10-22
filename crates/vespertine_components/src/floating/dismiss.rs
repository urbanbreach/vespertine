//! The window-scoped dismissal-layer registry (CMP-002 R6-R8).
//!
//! Every popup (menus, popovers, dialogs, select and combobox popups, hover cards, palettes)
//! registers one layer per open instance, either with [`register_layer`] (paired with
//! [`unregister_layer`]) or as a [`LayerRegistration`], which unregisters the layer when it is
//! dropped, so an owner released without closing its popup leaves no layer behind. Layers of a
//! window form a stack in registration order: the layer registered later sits above the earlier
//! one. Passive tooltips shown by the base hover mechanism do not register.
//!
//! **Escape ownership.** An unprevented window-level Escape reaches only the top layer's
//! handler, ahead of key bindings, root listeners and application shortcuts, and is consumed. A
//! handler answers with an [`EscapeOutcome`]: `Dismissed` when it closed (the layer unregisters
//! itself with [`unregister_layer`]) or `SteppedBack` when it only undid the innermost depth (a
//! submenu, a secondary panel, a non-empty search); a step-back keeps focus indicators hidden
//! until the next navigation key.
//!
//! Two focused owners see Escape before the stack does: a focused text input with an
//! uncommitted input-method composition, and, while focus is inside the top layer, an element on
//! the focus path whose key context contains [`ESCAPE_OWNER_CONTEXT`] (an inline draft that
//! cancels itself on Escape). The layer receives the next Escape once they are gone.
//!
//! **Dismissal by identity.** Every layer also carries its owner's full dismissal, so code other
//! than the owner can close it: [`dismiss_layer`] closes one layer and [`dismiss_all_layers`]
//! every layer of a window, the top one first (before a modal dialog opens, say, or when a
//! navigation replaces the screen). Unlike Escape, a dismissal never only steps back and runs no
//! Escape callback of the owner's caller.
//!
//! The registry also records each layer's painted bounds so that presses inside a layer above
//! another count as inside the lower one (see [`press_is_inside`]).

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
};
use vespertine::{
    App, Bounds, FocusHandle, Global, KeystrokeEvent, Pixels, Point, Subscription, Window, WindowId,
};

/// The key-context identifier with which a focused element inside the top layer claims Escape:
/// while focus is inside an element whose key context contains it, Escape reaches that
/// element's own handlers instead of the dismissal stack. The element must handle the Escape.
pub const ESCAPE_OWNER_CONTEXT: &str = "EscapeOwner";

/// Identity of a registered dismissal layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(u64);

/// What a layer's Escape handler did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeOutcome {
    /// The layer dismissed itself.
    Dismissed,
    /// The layer undid its innermost depth and stays open.
    SteppedBack,
    /// The layer consumed the Escape without changing anything (a dialog whose Escape
    /// dismissal is switched off, CMP-003 R10): it stays open and focus indicators are kept.
    Consumed,
}

/// A layer's Escape handler: runs the caller's Escape handler, then performs the dismissal or
/// the step-back.
pub type EscapeHandler = Rc<dyn Fn(&mut Window, &mut App) -> EscapeOutcome>;

/// A layer's full dismissal, run by [`dismiss_layer`]: closes the whole layer the way its owner
/// closes it (the layer unregisters itself and the owner learns that it closed), without a
/// step-back and without the Escape callbacks of the owner's caller.
pub type DismissHandler = Rc<dyn Fn(&mut Window, &mut App)>;

struct Entry {
    id: LayerId,
    escape: EscapeHandler,
    dismiss: DismissHandler,
    /// The layer's focus container: focus inside it is focus inside the layer.
    scope: Option<FocusHandle>,
    bounds: Option<Bounds<Pixels>>,
}

#[derive(Default)]
struct State {
    windows: HashMap<WindowId, Vec<Entry>>,
    next: u64,
    interceptor: Option<Subscription>,
}

impl State {
    fn remove(&mut self, window_id: WindowId, id: LayerId) -> bool {
        let Some(layers) = self.windows.get_mut(&window_id) else {
            return false;
        };
        let before = layers.len();
        layers.retain(|entry| entry.id != id);
        let removed = layers.len() != before;
        if layers.is_empty() {
            self.windows.remove(&window_id);
        }
        removed
    }
}

#[derive(Clone, Default)]
struct Shared(Rc<RefCell<State>>);

impl Global for Shared {}

fn shared(cx: &mut App) -> Shared {
    cx.default_global::<Shared>().clone()
}

fn read(cx: &App) -> Option<Shared> {
    cx.try_global::<Shared>().cloned()
}

/// Registers a layer above every layer already registered in `window` and returns its identity.
/// Registration is the moment the popup mounts; pair it with [`unregister_layer`] on close, or
/// use [`LayerRegistration`] to tie the layer to the lifetime of its owner.
pub fn register_layer(
    window: &Window,
    cx: &mut App,
    escape: EscapeHandler,
    dismiss: DismissHandler,
) -> LayerId {
    register(window, cx, escape, dismiss, None)
}

fn register(
    window: &Window,
    cx: &mut App,
    escape: EscapeHandler,
    dismiss: DismissHandler,
    scope: Option<FocusHandle>,
) -> LayerId {
    let shared = shared(cx);
    let open: Vec<WindowId> = cx
        .windows()
        .iter()
        .map(|handle| handle.window_id())
        .collect();
    let id = {
        let mut state = shared.0.borrow_mut();
        // Windows that closed take their layers with them.
        state
            .windows
            .retain(|window_id, _| open.contains(window_id));
        let id = LayerId(state.next);
        state.next += 1;
        state
            .windows
            .entry(window.window_handle().window_id())
            .or_default()
            .push(Entry {
                id,
                escape,
                dismiss,
                scope,
                bounds: None,
            });
        id
    };
    if shared.0.borrow().interceptor.is_none() {
        let handler = shared.clone();
        let subscription = cx.intercept_keystrokes(move |event, window, cx| {
            let keystroke = &event.keystroke;
            if keystroke.key != "escape" || keystroke.modifiers.modified() {
                return;
            }
            let top = handler
                .0
                .borrow()
                .windows
                .get(&window.window_handle().window_id())
                .and_then(|layers| layers.last())
                .map(|entry| (entry.escape.clone(), entry.scope.clone()));
            let Some((escape, scope)) = top else {
                return;
            };
            if focus_owns_escape(event, scope.as_ref(), window, cx) {
                return;
            }
            cx.stop_propagation();
            if escape(window, cx) == EscapeOutcome::SteppedBack {
                window.use_pointer_modality();
            }
        });
        shared.0.borrow_mut().interceptor = Some(subscription);
    }
    id
}

/// Whether a focused owner takes this Escape before the top layer: an input-method composition
/// in the focused input, or an [`ESCAPE_OWNER_CONTEXT`] element on the focus path while focus is
/// inside the top layer (`scope`).
fn focus_owns_escape(
    event: &KeystrokeEvent,
    scope: Option<&FocusHandle>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    if window.focused_input_is_composing(cx) {
        return true;
    }
    scope.is_some_and(|scope| scope.contains_focused(window, cx))
        && event
            .context_stack
            .iter()
            .any(|context| context.contains(ESCAPE_OWNER_CONTEXT))
}

/// Removes a layer (unmount, close or presence end). Returns whether it was registered.
pub fn unregister_layer(window: &Window, cx: &mut App, id: LayerId) -> bool {
    let Some(shared) = read(cx) else {
        return false;
    };
    shared
        .0
        .borrow_mut()
        .remove(window.window_handle().window_id(), id)
}

/// Dismisses layer `id` of `window` the way its owner closes it (see [`DismissHandler`]): the
/// whole layer closes where Escape might only step back (a submenu, a search) or be refused (a
/// dialog that ignores Escape). Returns whether `id` was registered in `window`.
///
/// The dismissal updates the layer's owner, so call it outside the owner's updates, as the
/// registry does for Escape.
pub fn dismiss_layer(window: &mut Window, cx: &mut App, id: LayerId) -> bool {
    let dismiss = read(cx).and_then(|shared| {
        shared
            .0
            .borrow()
            .windows
            .get(&window.window_handle().window_id())
            .and_then(|layers| layers.iter().find(|entry| entry.id == id))
            .map(|entry| entry.dismiss.clone())
    });
    // The registry is not borrowed while the owner closes: closing unregisters the layer.
    let Some(dismiss) = dismiss else {
        return false;
    };
    dismiss(window, cx);
    true
}

/// Dismisses every layer of `window`, the top one first (see [`dismiss_layer`]). A layer that a
/// dismissal above it already closed (a nested menu closing with its parent) is skipped.
pub fn dismiss_all_layers(window: &mut Window, cx: &mut App) {
    for id in layers(window, cx).into_iter().rev() {
        dismiss_layer(window, cx, id);
    }
}

/// A dismissal layer that stays registered for as long as this value lives. Dropping it
/// unregisters the layer, so a popup owner that is released while its popup is open leaves no
/// layer behind to swallow later Escapes.
pub struct LayerRegistration {
    id: LayerId,
    window: WindowId,
    registry: Weak<RefCell<State>>,
}

impl LayerRegistration {
    /// Registers a layer above every layer already registered in `window`.
    pub fn new(
        window: &Window,
        cx: &mut App,
        escape: EscapeHandler,
        dismiss: DismissHandler,
    ) -> Self {
        Self::scoped(window, cx, escape, dismiss, None)
    }

    /// Registers a layer whose focus container is `scope`.
    pub(super) fn scoped(
        window: &Window,
        cx: &mut App,
        escape: EscapeHandler,
        dismiss: DismissHandler,
        scope: Option<FocusHandle>,
    ) -> Self {
        let id = register(window, cx, escape, dismiss, scope);
        Self {
            id,
            window: window.window_handle().window_id(),
            registry: Rc::downgrade(&shared(cx).0),
        }
    }

    /// The layer's identity.
    pub fn id(&self) -> LayerId {
        self.id
    }
}

impl Drop for LayerRegistration {
    fn drop(&mut self) {
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        // A registration released while the registry itself removes entries (an entry's Escape
        // handler held it) belongs to an entry that is being removed already.
        if let Ok(mut state) = registry.try_borrow_mut() {
            state.remove(self.window, self.id);
        }
    }
}

/// The layers of `window` in registration order (bottom first).
pub fn layers(window: &Window, cx: &App) -> Vec<LayerId> {
    read(cx)
        .and_then(|shared| {
            shared
                .0
                .borrow()
                .windows
                .get(&window.window_handle().window_id())
                .map(|layers| layers.iter().map(|entry| entry.id).collect())
        })
        .unwrap_or_default()
}

/// The top layer of `window`, the one that owns Escape.
pub fn top_layer(window: &Window, cx: &App) -> Option<LayerId> {
    layers(window, cx).last().copied()
}

/// Whether `id` is the top layer of `window`.
pub fn is_top_layer(window: &Window, cx: &App, id: LayerId) -> bool {
    top_layer(window, cx) == Some(id)
}

/// Records the painted bounds of a layer (called every frame by its press probe).
pub fn set_layer_bounds(window: &Window, cx: &App, id: LayerId, bounds: Bounds<Pixels>) {
    if let Some(shared) = read(cx)
        && let Some(entry) = shared
            .0
            .borrow_mut()
            .windows
            .get_mut(&window.window_handle().window_id())
            .and_then(|layers| layers.iter_mut().find(|entry| entry.id == id))
    {
        entry.bounds = Some(bounds);
    }
}

/// Whether a press at `position` is inside layer `id` or inside any layer above it (a nested
/// layer is part of the one it was opened from). `None` when `id` is not registered.
pub fn press_is_inside(
    window: &Window,
    cx: &App,
    id: LayerId,
    position: Point<Pixels>,
) -> Option<bool> {
    let shared = read(cx)?;
    let state = shared.0.borrow();
    let layers = state.windows.get(&window.window_handle().window_id())?;
    let index = layers.iter().position(|entry| entry.id == id)?;
    Some(layers[index..].iter().any(|entry| {
        entry
            .bounds
            .is_some_and(|bounds| bounds.contains(&position))
    }))
}
