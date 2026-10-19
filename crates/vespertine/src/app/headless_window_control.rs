//! Window-level control of a [`HeadlessAppContext`]'s test windows: delivering platform input and
//! frame requests exactly as a platform event loop would, and observing what the window handed
//! to its platform. This is the surface an out-of-crate test host (see the `vespertine_test_host`
//! crate) builds on; none of it exists in production builds.

use crate::{
    AnyWindowHandle, App, Entity, HeadlessAppContext, Pixels, PlatformInput, Render,
    RequestFrameOptions, Result, Size, TestWindow, Window, WindowHandle, WindowOptions,
};
use anyhow::anyhow;

impl HeadlessAppContext {
    fn test_window(&self, window: AnyWindowHandle) -> Result<TestWindow> {
        let mut app = self.app.borrow_mut();
        let window = app
            .windows
            .get_mut(window.window_id())
            .and_then(|window| window.as_deref_mut())
            .ok_or_else(|| anyhow!("window is not open"))?;
        window
            .platform_window
            .as_test()
            .map(|window| window.clone())
            .ok_or_else(|| anyhow!("window is not a test window"))
    }

    /// Opens a window with fully specified options (bounds, focus, ...).
    ///
    /// [`HeadlessAppContext::open_window`] opens an unfocused window at the origin; hosts that
    /// need an offscreen placement, or a focused window that receives keyboard input, use this.
    pub fn open_window_with_options<V: Render + 'static>(
        &mut self,
        options: WindowOptions,
        build_root: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Result<WindowHandle<V>> {
        self.app.borrow_mut().open_window(options, build_root)
    }

    /// Delivers a platform input event to the window exactly as the platform's event loop
    /// would (hit testing, focus routing and listeners included). Returns whether the window
    /// handled it (did not let it propagate). Does not run executors or draw.
    pub fn simulate_platform_input(
        &mut self,
        window: AnyWindowHandle,
        input: PlatformInput,
    ) -> Result<bool> {
        Ok(self.test_window(window)?.simulate_input(input))
    }

    /// Delivers a frame request to the window as the platform's frame source would. The
    /// window draws when it is invalid or `options.force_render` is set, and presents to its
    /// renderer.
    pub fn request_frame(
        &mut self,
        window: AnyWindowHandle,
        options: RequestFrameOptions,
    ) -> Result<()> {
        self.test_window(window)?.simulate_frame_request(options);
        Ok(())
    }

    /// Whether the window was invalidated since its last draw.
    pub fn window_needs_draw(&mut self, window: AnyWindowHandle) -> Result<bool> {
        self.update_window(window, |_, window, _| window.needs_draw())
    }

    /// Resizes the window as a user or compositor would, preserving its scale factor.
    pub fn simulate_window_resize(
        &mut self,
        window: AnyWindowHandle,
        size: Size<Pixels>,
    ) -> Result<()> {
        self.test_window(window)?.simulate_resize(size);
        Ok(())
    }

    /// Moves the window to a display with another scale factor, preserving its logical bounds.
    pub fn simulate_window_scale_factor(
        &mut self,
        window: AnyWindowHandle,
        scale_factor: f32,
    ) -> Result<()> {
        self.test_window(window)?
            .simulate_scale_factor_change(scale_factor);
        Ok(())
    }

    /// The window's scale factor.
    pub fn window_scale_factor(&mut self, window: AnyWindowHandle) -> Result<f32> {
        self.update_window(window, |_, window, _| window.scale_factor())
    }

    /// Connects a simulated assistive technology to the window. See
    /// [`TestWindow::simulate_accessibility_activation`].
    pub fn simulate_accessibility_activation(&mut self, window: AnyWindowHandle) -> Result<bool> {
        Ok(self
            .test_window(window)?
            .simulate_accessibility_activation())
    }

    /// The accessibility tree update most recently delivered to the window's platform.
    pub fn last_accessibility_update(
        &mut self,
        window: AnyWindowHandle,
    ) -> Result<Option<accesskit::TreeUpdate>> {
        Ok(self.test_window(window)?.last_accessibility_update())
    }
}
