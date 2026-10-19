//! The window-scoped dismissal-layer registry (CMP-002 R6-R8).
//!
//! Every popup (menus, popovers, dialogs, select and combobox popups, hover cards, palettes)
//! registers one layer per open instance with [`register_layer`]. Layers of a window form a
//! stack in registration order: the layer registered later sits above the earlier one. Passive
//! tooltips shown by the base hover mechanism do not register.
//!
//! **Escape ownership.** An unprevented window-level Escape reaches only the top layer's
//! handler, ahead of key bindings, root listeners and application shortcuts, and is consumed. A
//! handler answers with an [`EscapeOutcome`]: `Dismissed` when it closed (the layer unregisters
//! itself with [`unregister_layer`]) or `SteppedBack` when it only undid the innermost depth (a
//! submenu, a secondary panel, a non-empty search); a step-back keeps focus indicators hidden
//! until the next navigation key.
//!
//! The registry also records each layer's painted bounds so that presses inside a layer above
//! another count as inside the lower one (see [`press_is_inside`]).

use std::{cell::RefCell, collections::HashMap, rc::Rc};
use vespertine::{App, Bounds, Global, Pixels, Point, Subscription, Window, WindowId};

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
}

/// A layer's Escape handler: runs the caller's Escape handler, then performs the dismissal or
/// the step-back.
pub type EscapeHandler = Rc<dyn Fn(&mut Window, &mut App) -> EscapeOutcome>;

struct Entry {
    id: LayerId,
    escape: EscapeHandler,
    bounds: Option<Bounds<Pixels>>,
}

#[derive(Default)]
struct State {
    windows: HashMap<WindowId, Vec<Entry>>,
    next: u64,
    interceptor: Option<Subscription>,
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
/// Registration is the moment the popup mounts; pair it with [`unregister_layer`] on close.
pub fn register_layer(window: &Window, cx: &mut App, escape: EscapeHandler) -> LayerId {
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
                .map(|entry| entry.escape.clone());
            if let Some(escape) = top {
                cx.stop_propagation();
                if escape(window, cx) == EscapeOutcome::SteppedBack {
                    window.use_pointer_modality();
                }
            }
        });
        shared.0.borrow_mut().interceptor = Some(subscription);
    }
    id
}

/// Removes a layer (unmount, close or presence end). Returns whether it was registered.
pub fn unregister_layer(window: &Window, cx: &mut App, id: LayerId) -> bool {
    let Some(shared) = read(cx) else {
        return false;
    };
    let mut state = shared.0.borrow_mut();
    let window_id = window.window_handle().window_id();
    let Some(layers) = state.windows.get_mut(&window_id) else {
        return false;
    };
    let before = layers.len();
    layers.retain(|entry| entry.id != id);
    let removed = layers.len() != before;
    if layers.is_empty() {
        state.windows.remove(&window_id);
    }
    removed
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
