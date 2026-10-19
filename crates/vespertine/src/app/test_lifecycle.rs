//! Test hooks for the application lifecycle (PLT-008): window geometry modes, close
//! requests, quit requests, and the platform's activation mechanism.

use crate::{
    ActivationRequest, AnyWindowHandle, Bounds, DisplayId, Pixels, Point, TestActivation,
    TestAppContext, TestDisplay, WindowMode,
};

impl TestAppContext {
    /// Delivers an incoming URL list as the platform's activation mechanism would (on
    /// Linux the `Open` call of `org.freedesktop.Application`).
    pub fn simulate_open_urls(&self, urls: Vec<String>) {
        self.test_platform().simulate_open_urls(urls);
        self.background_executor.run_until_parked();
    }

    /// Delivers a reopen notification (on Linux an `Activate` call).
    pub fn simulate_reopen(&self) {
        self.test_platform().simulate_reopen();
        self.background_executor.run_until_parked();
    }

    /// Delivers an activation request, as the platform does after the reopen or open-URLs
    /// notification of a call that carried platform data.
    pub fn simulate_activation_request(&self, request: ActivationRequest) {
        self.test_platform().simulate_activation_request(request);
        self.background_executor.run_until_parked();
    }

    /// Delivers an activated desktop action (on Linux an `ActivateAction` call).
    pub fn simulate_activate_action(&self, action: &str) {
        self.test_platform().simulate_activate_action(action);
        self.background_executor.run_until_parked();
    }

    /// Delivers a termination request, as the platform does for `SIGTERM` on Linux.
    pub fn simulate_quit_request(&self) {
        self.test_platform().simulate_quit_request();
    }

    /// How many times the application asked the platform to stop the event loop.
    pub fn platform_quit_count(&self) -> usize {
        self.test_platform().quit_count()
    }

    /// Simulates a close request for the window from the compositor or window manager: the
    /// window's close decision runs and the window is removed only when it allows it.
    /// Returns whether the window was closed. Enumerating displays while the request is
    /// handled fails the test.
    pub fn simulate_window_close_request(&self, window: AnyWindowHandle) -> bool {
        let closed = self.test_window(window).simulate_close_request();
        self.background_executor.run_until_parked();
        closed
    }

    /// Simulates the window manager maximizing the window to its display's work area, or
    /// restoring it.
    pub fn simulate_window_maximize(&self, window: AnyWindowHandle, maximized: bool) {
        self.test_window(window).simulate_maximize(maximized);
        self.background_executor.run_until_parked();
    }

    /// Simulates the window manager making the window fullscreen, or restoring it.
    pub fn simulate_window_fullscreen(&self, window: AnyWindowHandle, fullscreen: bool) {
        self.test_window(window).simulate_fullscreen(fullscreen);
        self.background_executor.run_until_parked();
    }

    /// Simulates the window being moved to `origin`.
    pub fn simulate_window_move(&self, window: AnyWindowHandle, origin: Point<Pixels>) {
        self.test_window(window).simulate_move(origin);
        self.background_executor.run_until_parked();
    }

    /// The mode a test window is in.
    pub fn window_mode(&self, window: AnyWindowHandle) -> WindowMode {
        self.test_window(window).mode()
    }

    /// The ordinary bounds a test window returns to when it leaves maximized or fullscreen.
    pub fn window_ordinary_bounds(&self, window: AnyWindowHandle) -> Bounds<Pixels> {
        self.test_window(window).ordinary_bounds()
    }

    /// Whether a test window is minimized.
    pub fn is_window_minimized(&self, window: AnyWindowHandle) -> bool {
        self.test_window(window).is_minimized()
    }

    /// Minimizes a test window as the window manager would.
    pub fn simulate_window_minimize(&self, window: AnyWindowHandle) {
        crate::PlatformWindow::minimize(&self.test_window(window));
    }

    /// How a test window was asked to come forward, in order.
    pub fn window_activations(&self, window: AnyWindowHandle) -> Vec<TestActivation> {
        self.test_window(window).activations()
    }

    /// Connects a display with the given identity, bounds and work area.
    pub fn simulate_display_connected(
        &self,
        id: DisplayId,
        uuid: uuid::Uuid,
        bounds: Bounds<Pixels>,
        visible_bounds: Bounds<Pixels>,
    ) {
        self.test_platform()
            .simulate_display_connected(TestDisplay::with_geometry(
                id,
                uuid,
                bounds,
                visible_bounds,
            ));
    }

    /// Changes the bounds and work area of a connected display.
    pub fn simulate_display_geometry_change(
        &self,
        id: DisplayId,
        uuid: uuid::Uuid,
        bounds: Bounds<Pixels>,
        visible_bounds: Bounds<Pixels>,
    ) {
        self.test_platform()
            .simulate_display_replaced(TestDisplay::with_geometry(
                id,
                uuid,
                bounds,
                visible_bounds,
            ));
    }
}
