//! Safe focus restoration and pane-level focus scopes (INP-007 R2).
//!
//! Handles can outlive the element they were tracked on: an editor is closed,
//! a dialog is dismissed, a tab is detached. Moving focus back to such a handle
//! would leave focus on nothing (or on a dropped element). [`Window::restore_focus`]
//! therefore checks, on the next frame, that the handle's element is mounted in
//! the rendered frame and otherwise falls back to a handle that is, such as the
//! pane or window root.

use crate::{App, FocusHandle, InteractiveElement, WeakFocusHandle, Window};

impl Window {
    /// Requests that focus return to `target`, taking effect on the next
    /// frame and only if the element tracking `target` is mounted in the
    /// rendered frame at that time.
    ///
    /// Otherwise `fallback` (a valid, mounted root such as the pane or window
    /// root, never a stale editor) receives focus. A handle whose element has
    /// been dropped never receives focus. When neither handle can be focused
    /// the focus is left unchanged.
    pub fn restore_focus(&self, target: &WeakFocusHandle, fallback: &WeakFocusHandle) {
        let target = target.clone();
        let fallback = fallback.clone();
        self.on_next_frame(move |window, cx| {
            let mounted = |weak: &WeakFocusHandle| {
                weak.upgrade()
                    .filter(|handle| window.is_focus_mounted(handle))
            };
            if let Some(handle) = mounted(&target).or_else(|| mounted(&fallback)) {
                window.focus(&handle, cx);
            }
        });
    }

    /// Whether the element tracking `handle` is present in the rendered frame.
    pub fn is_focus_mounted(&self, handle: &FocusHandle) -> bool {
        self.rendered_frame
            .dispatch_tree
            .focusable_node_id(handle.id())
            .is_some()
    }
}

/// Makes an element a pane-level focus scope.
pub trait FocusScopeExt: InteractiveElement + Sized {
    /// Tracks `scope` on this element and makes the element establish itself
    /// as the focus fallback for pointer presses inside it:
    ///
    /// - in the capture phase of a press the scope takes focus when it does
    ///   not already contain focus; inputs inside it may still claim focus
    ///   while the press bubbles (including listeners that stop propagation),
    ///   and a press inside an already focused input never blurs it first;
    /// - after the press has been handled, a deferred check focuses the scope
    ///   if nothing is focused.
    fn focus_scope(self, scope: &FocusHandle) -> Self {
        let scope_handle = scope.clone();
        self.track_focus(scope)
            .capture_any_mouse_down(move |_, window, cx| {
                if !scope_handle.contains_focused(window, cx) {
                    window.focus(&scope_handle, cx);
                }
                let scope_handle = scope_handle.clone();
                window.defer(cx, move |window, cx| {
                    if window.focused(cx).is_none() {
                        window.focus(&scope_handle, cx);
                    }
                });
            })
    }
}

impl<E: InteractiveElement> FocusScopeExt for E {}

/// Remembers which element of a pane had focus so focus can return to it
/// later (for example when the pane is activated again).
///
/// The remembered origin is only replaced by a real element of the pane: when
/// the focused element is being detached, focus falls back to the pane's own
/// scope handle, and that fallback never overwrites the origin.
pub struct FocusMemory {
    scope: WeakFocusHandle,
    origin: Option<WeakFocusHandle>,
}

impl FocusMemory {
    /// A memory for the pane whose scope handle is `scope`.
    pub fn new(scope: &FocusHandle) -> Self {
        Self {
            scope: scope.downgrade(),
            origin: None,
        }
    }

    /// The currently remembered origin, if its handle is still alive.
    pub fn origin(&self) -> Option<FocusHandle> {
        self.origin.as_ref().and_then(WeakFocusHandle::upgrade)
    }

    /// Updates the remembered origin from the window's current focus. Call it
    /// whenever focus may have moved (for example from a focus-in listener of
    /// the scope). Focus outside the pane, or on the scope handle itself,
    /// keeps the previous origin.
    pub fn remember(&mut self, window: &Window, cx: &App) {
        let Some(scope) = self.scope.upgrade() else {
            return;
        };
        let Some(focused) = window.focused(cx) else {
            return;
        };
        if focused != scope && scope.contains(&focused, window) {
            self.origin = Some(focused.downgrade());
        }
    }

    /// Restores focus to the remembered origin, unless a mounted focus
    /// elsewhere (for example in a dialog) takes precedence. When the origin is
    /// gone or unmounted the pane's scope handle receives focus instead, as
    /// with [`Window::restore_focus`]. Takes effect on the next frame.
    pub fn restore(&self, window: &mut Window, cx: &App) {
        let Some(scope) = self.scope.upgrade() else {
            return;
        };
        if let Some(focused) = window.focused(cx)
            && focused != scope
            && window.is_focus_mounted(&focused)
            && !scope.contains(&focused, window)
        {
            return;
        }
        let origin = self.origin.clone().unwrap_or_else(|| scope.downgrade());
        window.restore_focus(&origin, &scope.downgrade());
    }
}
