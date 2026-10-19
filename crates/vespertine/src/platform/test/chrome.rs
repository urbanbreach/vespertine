//! Scripted window chrome behavior for [`TestWindow`]: decoration mode, tiling, supported
//! controls, background effect support, and a log of the native requests the window received.

use crate::{
    Decorations, Pixels, Point, ResizeEdge, TestWindow, WindowBackgroundAppearance,
    WindowBackgroundCapabilities, WindowButtonLayout, WindowControls, WindowDecorations,
};

/// A native window operation requested from a [`TestWindow`].
#[derive(Clone, Debug, PartialEq)]
pub enum TestChromeRequest {
    /// `start_window_move`.
    Move,
    /// `start_window_resize` for the given edge.
    Resize(ResizeEdge),
    /// `show_window_menu` at the given window position.
    WindowMenu(Point<Pixels>),
    /// `minimize`.
    Minimize,
    /// `zoom`, which toggles between maximized and restored.
    ToggleMaximize,
    /// `request_decorations`.
    RequestDecorations(WindowDecorations),
    /// `set_client_inset` with the inset the window pushed.
    ClientInset(Pixels),
}

pub(crate) struct TestChromeState {
    pub(crate) decorations: Decorations,
    pub(crate) controls: WindowControls,
    pub(crate) capabilities: WindowBackgroundCapabilities,
    pub(crate) background: WindowBackgroundAppearance,
    pub(crate) requests: Vec<TestChromeRequest>,
    pub(crate) button_layout_callback: Option<Box<dyn FnMut()>>,
}

impl Default for TestChromeState {
    fn default() -> Self {
        Self {
            decorations: Decorations::Server,
            controls: WindowControls::default(),
            // Like a Wayland compositor without the KDE blur protocol.
            capabilities: WindowBackgroundCapabilities {
                transparency: true,
                blur: false,
            },
            background: WindowBackgroundAppearance::Opaque,
            requests: Vec::new(),
            button_layout_callback: None,
        }
    }
}

impl TestWindow {
    /// Sets the decoration mode (and tiling) the window reports.
    pub fn set_decorations(&self, decorations: Decorations) {
        self.0.lock().chrome.decorations = decorations;
    }

    /// Sets the window controls the window reports as supported.
    pub fn set_window_controls(&self, controls: WindowControls) {
        self.0.lock().chrome.controls = controls;
    }

    /// Sets the background effects the window reports as available.
    pub fn set_background_capabilities(&self, capabilities: WindowBackgroundCapabilities) {
        self.0.lock().chrome.capabilities = capabilities;
    }

    /// Makes the window maximized or restores it, as the window manager would.
    pub fn set_maximized(&self, maximized: bool) {
        self.simulate_maximize(maximized);
    }

    /// The native requests received so far, in order.
    pub fn chrome_requests(&self) -> Vec<TestChromeRequest> {
        self.0.lock().chrome.requests.clone()
    }

    /// The background appearance last passed to `set_background_appearance`.
    pub fn requested_background(&self) -> WindowBackgroundAppearance {
        self.0.lock().chrome.background
    }

    /// Changes the desktop button layout and notifies the window, as the settings portal would.
    pub fn simulate_button_layout_change(&self, layout: Option<WindowButtonLayout>) {
        let (platform, callback) = {
            let mut state = self.0.lock();
            (
                state.platform.upgrade(),
                state.chrome.button_layout_callback.take(),
            )
        };
        if let Some(platform) = platform {
            *platform.button_layout.borrow_mut() = layout;
        }
        if let Some(mut callback) = callback {
            callback();
            self.0.lock().chrome.button_layout_callback = Some(callback);
        }
    }

    /// Forgets the native requests received so far.
    pub fn clear_chrome_requests(&self) {
        self.0.lock().chrome.requests.clear();
    }

    pub(crate) fn record_chrome_request(&self, request: TestChromeRequest) {
        self.0.lock().chrome.requests.push(request);
    }
}
