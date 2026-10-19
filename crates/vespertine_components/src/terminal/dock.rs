//! The bottom dock (CMP-021 R21): a terminal at the bottom of a window sharing one measured
//! height with the content above it.

use vespertine::{
    AnyElement, App, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
};

use super::geometry::{DOCK_MIN_HEIGHT, DOCK_TOLERANCE, dock_height};
use super::panel::TerminalPanel;

/// The geometry record of one dock frame, shared by the terminal and the content above it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockGeometry {
    /// The window height the dock was laid out for.
    pub window_height: f32,
    /// The height the host asked for.
    pub requested: f32,
    /// The terminal height: `requested` clamped to `[160, max(55% of window, 160)]`.
    pub terminal_height: f32,
    /// The height the content above receives, as both layout size and hitbox size: window
    /// height minus the terminal height.
    pub content_height: f32,
    /// The terminal height measured in the previous frame, when it has been painted.
    pub measured_height: Option<f32>,
}

/// A window-height layout with the terminal anchored to the bottom and `content` above it.
pub struct BottomDock {
    terminal: Entity<TerminalPanel>,
    content: Box<dyn Fn(&mut Window, &mut App) -> AnyElement>,
    requested: f32,
    geometry: Option<DockGeometry>,
}

impl BottomDock {
    /// A dock hosting `terminal` below the element built by `content`, requesting the minimum
    /// height until [`set_height`](Self::set_height).
    pub fn new(
        terminal: Entity<TerminalPanel>,
        content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        Self {
            terminal,
            content: Box::new(content),
            requested: DOCK_MIN_HEIGHT,
            geometry: None,
        }
    }

    /// The same dock requesting `height` logical pixels.
    pub fn with_height(mut self, height: f32) -> Self {
        self.requested = height;
        self
    }

    /// Requests a terminal height in logical pixels; it is clamped when laid out and a
    /// non-finite request becomes the minimum.
    pub fn set_height(&mut self, height: f32, cx: &mut Context<Self>) {
        self.requested = height;
        cx.notify();
    }

    /// The height last requested.
    pub fn requested_height(&self) -> f32 {
        self.requested
    }

    /// The geometry record of the last rendered frame.
    pub fn geometry(&self) -> Option<DockGeometry> {
        self.geometry
    }

    /// The hosted terminal.
    pub fn terminal(&self) -> &Entity<TerminalPanel> {
        &self.terminal
    }
}

impl Render for BottomDock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let window_height = f32::from(window.viewport_size().height);
        let terminal_height = dock_height(self.requested, window_height);
        let content_height = (window_height - terminal_height).max(0.0);
        let measured_height = self
            .terminal
            .read(cx)
            .geometry()
            .map(|geometry| f32::from(geometry.bounds.size.height));
        // The previous frame measured another height than the one requested: ask for another
        // frame so consumers re-read the shared record.
        if measured_height
            .is_some_and(|measured| (measured - terminal_height).abs() > DOCK_TOLERANCE)
        {
            window.request_animation_frame();
        }
        self.geometry = Some(DockGeometry {
            window_height,
            requested: self.requested,
            terminal_height,
            content_height,
            measured_height,
        });
        let content = (self.content)(window, cx);
        div()
            .size_full()
            .flex_col()
            .flex()
            .child(
                div()
                    .w_full()
                    .h(px(content_height))
                    .flex_none()
                    .overflow_hidden()
                    .child(content),
            )
            .child(
                div()
                    .w_full()
                    .h(px(terminal_height))
                    .flex_none()
                    .child(self.terminal.clone()),
            )
    }
}
