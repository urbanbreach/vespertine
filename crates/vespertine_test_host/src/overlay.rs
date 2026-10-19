//! Frame-overlay controls (profiler builds only).

use crate::TestHost;
use vespertine::DebugFrameOverlayMode;

impl TestHost {
    /// The current frame-overlay mode.
    pub fn frame_overlay_mode(&mut self) -> DebugFrameOverlayMode {
        self.cx
            .update_window(self.window, |_, window, _| {
                window.debug_frame_overlay_mode()
            })
            .expect("the host window is open for the host's lifetime")
    }

    /// Sets the frame-overlay mode and requests a frame.
    pub fn set_frame_overlay_mode(&mut self, mode: DebugFrameOverlayMode) {
        self.cx
            .update_window(self.window, |_, window, _| {
                window.set_debug_frame_overlay_mode(mode)
            })
            .expect("the host window is open for the host's lifetime");
        self.invalidate();
    }

    /// Advances the overlay mode through hidden, minimal and full.
    pub fn cycle_frame_overlay_mode(&mut self) {
        self.cx
            .update_window(self.window, |_, window, _| {
                window.cycle_debug_frame_overlay_mode()
            })
            .expect("the host window is open for the host's lifetime");
        self.invalidate();
    }

    /// Resets the overlay: the mode returns to hidden and the collected statistics are cleared.
    pub fn reset_frame_overlay(&mut self) {
        self.cx
            .update_window(self.window, |_, window, _| {
                window.set_debug_frame_overlay_mode(DebugFrameOverlayMode::Hidden);
                window.reset_debug_frame_overlay_stats();
            })
            .expect("the host window is open for the host's lifetime");
        self.invalidate();
    }
}
