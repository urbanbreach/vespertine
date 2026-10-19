//! Window close interception and window-level geometry observers (PLT-008 R2, R5).
//!
//! Every request to close a window is routed through one close decision before anything is
//! destroyed: the compositor's close request (xdg-toplevel close, `WM_DELETE_WINDOW`), the
//! caption close control, [`Window::request_close`], and the close shortcut. The decision
//! answers synchronously. An asynchronous save vetoes first, saves as ordinary asynchronous
//! work, and issues a second [`Window::request_close`] when it finishes, which the decision
//! lets through because the application state says the document is saved.
//!
//! The decision and the removal that follows it must not enumerate displays or otherwise
//! re-enter platform state that is borrowed while the platform delivers the request. Use
//! [`Window::display`] and [`Window::bounds`], which answer from values the window and the
//! app cached earlier.

use std::rc::Rc;

use crate::{AnyWindowHandle, App, PlatformWindow, Subscription, Window};

/// A window's close decision: returns whether the close may proceed.
pub(crate) type CloseDecision = Rc<dyn Fn(&mut Window, &mut App) -> bool>;

/// Routes the platform's close requests for a window through its close decision.
pub(super) fn install_close_request_handler(
    platform_window: &mut dyn PlatformWindow,
    handle: AnyWindowHandle,
    cx: &App,
) {
    let mut cx = cx.to_async();
    platform_window.on_should_close(Box::new(move || {
        handle
            .update(&mut cx, |_, window, cx| window.decide_close(cx))
            .unwrap_or(true)
    }));
}

impl Window {
    /// Registers this window's close decision, replacing any previous one.
    ///
    /// The callback runs for every request to close the window and returns synchronously:
    /// `true` lets the close proceed, `false` vetoes it and the window stays fully intact
    /// (content, focus, size and state). To save asynchronously, return `false`, run the
    /// save as ordinary async work and call [`Window::request_close`] again when it is done,
    /// with application state (for example "already saved") making the callback return
    /// `true` that time. If the user cancels the save nothing further is issued.
    ///
    /// The callback and the removal that follows must not enumerate displays; see the
    /// [module documentation](self).
    pub fn on_window_should_close(
        &self,
        _cx: &App,
        f: impl Fn(&mut Window, &mut App) -> bool + 'static,
    ) {
        *self.close_decision.borrow_mut() = Some(Rc::new(f));
    }

    pub(crate) fn decide_close(&mut self, cx: &mut App) -> bool {
        let decision = self.close_decision.borrow().clone();
        decision.is_none_or(|decision| decision(self, cx))
    }

    /// Requests that this window close, consulting the close decision registered with
    /// [`Window::on_window_should_close`]. When the decision allows it (or none is
    /// registered) the window is removed. Returns whether the window is being removed.
    ///
    /// [`Window::remove_window`] closes without consulting the decision.
    pub fn request_close(&mut self, cx: &mut App) -> bool {
        let allow = self.decide_close(cx);
        if allow {
            self.remove_window();
        }
        allow
    }

    /// Registers a callback invoked when the window's bounds change (resize or move). In
    /// maximized and fullscreen states [`Window::window_bounds`] keeps returning the
    /// unchanged ordinary bounds to return to.
    pub fn observe_window_bounds(
        &mut self,
        mut callback: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let (subscription, activate) = self.bounds_observers.insert(
            (),
            Box::new(move |window, cx| {
                callback(window, cx);
                true
            }),
        );
        activate();
        subscription
    }

    /// Registers a callback invoked when the window is activated or deactivated
    /// (focused or unfocused); read the state with [`Window::is_window_active`].
    pub fn observe_window_activation(
        &mut self,
        mut callback: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let (subscription, activate) = self.activation_observers.insert(
            (),
            Box::new(move |window, cx| {
                callback(window, cx);
                true
            }),
        );
        activate();
        subscription
    }
}
