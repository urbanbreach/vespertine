//! Opening and closing focus of a dismissal layer (CMP-002 R9-R11).

use super::dismiss::{EscapeHandler, LayerId, register_layer, unregister_layer};
use std::rc::Rc;
use vespertine::{App, FocusHandle, Window};

/// What a focus callback or target resolves to (R11).
#[derive(Clone, Debug)]
pub enum FocusPick {
    /// Use the component's default (the spec's `undefined`).
    Default,
    /// Do not move focus (the spec's `null` / `false`).
    Suppress,
    /// Focus this element.
    Handle(FocusHandle),
}

/// The shared focus-target contract used by initial focus, final focus and the dialog parts: a
/// focus handle, a function returning a [`FocusPick`], the component default or suppression.
#[derive(Clone, Default)]
pub enum FocusTarget {
    /// The component default.
    #[default]
    Default,
    /// Do not move focus.
    Suppress,
    /// Focus this handle.
    Handle(FocusHandle),
    /// Decide when needed. Receives the focus owner captured when the layer opened.
    With(Rc<dyn Fn(Option<&FocusHandle>, &mut Window, &mut App) -> FocusPick>),
}

impl FocusTarget {
    /// Resolves the target; `prior` is the focus captured at open time.
    pub fn pick(
        &self,
        prior: Option<&FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> FocusPick {
        match self {
            FocusTarget::Default => FocusPick::Default,
            FocusTarget::Suppress => FocusPick::Suppress,
            FocusTarget::Handle(handle) => FocusPick::Handle(handle.clone()),
            FocusTarget::With(callback) => callback(prior, window, cx),
        }
    }
}

impl From<FocusHandle> for FocusTarget {
    fn from(handle: FocusHandle) -> Self {
        FocusTarget::Handle(handle)
    }
}

/// Everything a layer needs from its caller to manage focus and Escape.
pub struct LayerOptions {
    /// Runs the caller's Escape handler, then dismisses or steps back (R7, R8).
    pub escape: EscapeHandler,
    /// The layer's own focus handle (its container). Focus inside it counts as "inside the
    /// layer" when closing, and it is the default initial focus target.
    pub layer_focus: FocusHandle,
    /// The owning trigger, the fallback when the captured focus owner is gone.
    pub trigger: Option<FocusHandle>,
    /// Initial focus (R9); [`FocusTarget::Default`] focuses `layer_focus`.
    pub initial_focus: FocusTarget,
    /// Closing focus (R10).
    pub final_focus: FocusTarget,
    /// Default restoration order: the trigger before the captured prior owner (a dialog,
    /// CMP-003 R6 and R15) instead of the other way round (CMP-002 R10).
    pub trigger_first: bool,
}

/// An open layer: its registry identity and the focus bookkeeping. Created by [`open_layer`] and
/// ended by [`LayerHandle::close`].
#[derive(Clone)]
pub struct LayerHandle {
    id: LayerId,
    prior: Option<FocusHandle>,
    options: Rc<LayerOptionsInner>,
}

struct LayerOptionsInner {
    layer_focus: FocusHandle,
    trigger: Option<FocusHandle>,
    final_focus: FocusTarget,
    trigger_first: bool,
}

/// Opens a layer: captures the current focus owner (before the layer's elements mount),
/// registers it on top of the window's stack and schedules the opening focus for after its
/// subtree is attached to the dispatch tree (the deferred layer is not attached on its first
/// frame, so focus moves two frames later, as the base popover menu does).
pub fn open_layer(window: &mut Window, cx: &mut App, options: LayerOptions) -> LayerHandle {
    let prior = window.focused(cx);
    let id = register_layer(window, cx, options.escape);
    let handle = LayerHandle {
        id,
        prior,
        options: Rc::new(LayerOptionsInner {
            layer_focus: options.layer_focus,
            trigger: options.trigger,
            final_focus: options.final_focus,
            trigger_first: options.trigger_first,
        }),
    };
    let initial = options.initial_focus;
    let focus_handle = handle.clone();
    window.on_next_frame(move |window, _cx| {
        window.on_next_frame(move |window, cx| {
            if !is_layer_registered(window, cx, focus_handle.id) {
                return;
            }
            let target = match initial.pick(focus_handle.prior.as_ref(), window, cx) {
                FocusPick::Default => Some(focus_handle.options.layer_focus.clone()),
                FocusPick::Suppress => None,
                FocusPick::Handle(handle) => Some(handle),
            };
            if let Some(target) = target {
                target.focus(window, cx);
            }
        });
    });
    handle
}

fn is_layer_registered(window: &Window, cx: &App, id: LayerId) -> bool {
    super::dismiss::layers(window, cx).contains(&id)
}

impl LayerHandle {
    /// The layer's registry identity.
    pub fn id(&self) -> LayerId {
        self.id
    }

    /// The focus owner captured when the layer opened.
    pub fn prior_focus(&self) -> Option<&FocusHandle> {
        self.prior.as_ref()
    }

    /// Ends the layer: unregisters it (Escape stops reaching it at once) and restores focus
    /// after the current event has finished, so a press that moved focus elsewhere keeps it.
    /// Returns false (and does nothing more) when the layer was already closed.
    pub fn close(&self, window: &mut Window, cx: &mut App) -> bool {
        if !unregister_layer(window, cx, self.id) {
            return false;
        }
        let this = self.clone();
        window.defer(cx, move |window, cx| this.restore_focus(window, cx));
        true
    }

    /// R10. `final_focus` first: a handle or a function result always focuses; suppression
    /// leaves focus alone. Without one (default), focus returns to the captured owner only if
    /// focus is still inside the layer or nowhere; when that owner is no longer mounted the
    /// fallback is the owning trigger if mounted, otherwise the window's default (no focus).
    fn restore_focus(&self, window: &mut Window, cx: &mut App) {
        match self
            .options
            .final_focus
            .pick(self.prior.as_ref(), window, cx)
        {
            FocusPick::Handle(handle) => {
                handle.focus(window, cx);
                return;
            }
            FocusPick::Suppress => return,
            FocusPick::Default => {}
        }
        let layer = &self.options.layer_focus;
        let still_here = match window.focused(cx) {
            None => true,
            Some(focused) => focused == *layer || layer.contains(&focused, window),
        };
        if !still_here {
            return;
        }
        let (first, second) = if self.options.trigger_first {
            (self.options.trigger.as_ref(), self.prior.as_ref())
        } else {
            (self.prior.as_ref(), self.options.trigger.as_ref())
        };
        let target = first
            .into_iter()
            .chain(second)
            .find(|handle| handle.is_mounted(window));
        match target {
            Some(target) => target.clone().focus(window, cx),
            None => window.blur(cx),
        }
    }
}
