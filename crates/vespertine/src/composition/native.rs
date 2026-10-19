//! Platform-facing types of native composition: the opaque device/layer/generation value,
//! the native attachment interface and the input-forwarding preference (PLT-001 R10, R12,
//! R18).

use std::{ffi::c_void, ptr::NonNull};

use anyhow::Result;

use crate::{Bounds, DevicePixels, Point};

/// An opaque handle to the display-server connection the window's surfaces live on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NativeDevice {
    /// A `wl_display` pointer.
    Wayland {
        /// The `wl_display*` of the window's connection.
        display: NonNull<c_void>,
    },
    /// An X11 connection (`xcb_connection_t*`).
    X11 {
        /// The connection pointer.
        connection: NonNull<c_void>,
    },
    /// The in-memory test platform.
    Test {
        /// A test-chosen value.
        id: u64,
    },
}

/// An opaque handle to one compositor-level surface: the window's own surface or a native
/// attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NativeLayer {
    /// A `wl_surface` pointer.
    Wayland {
        /// The `wl_surface*`.
        surface: NonNull<c_void>,
    },
    /// An X11 window.
    X11 {
        /// The window id.
        window: u32,
    },
    /// The in-memory test platform.
    Test {
        /// A test-chosen value.
        id: u64,
    },
}

/// The platform handles native producers attach to, together with the generation that tells
/// them when those handles went stale.
///
/// The generation is a counter that changes on every compositor-surface recreation: device
/// recovery, overlay renderer recreation from a new device, X11 overlay window recreation and
/// Wayland role-object recreation. Handles obtained before a change are stale and must be
/// obtained again; embedders learn about the change through
/// [`NativeAttachment::on_recreated`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeComposition {
    /// The display-server connection.
    pub device: NativeDevice,
    /// The window's own surface.
    pub layer: NativeLayer,
    /// The recreation counter.
    pub generation: u64,
}

/// Whether the framework forwards pointer input over a native surface to its producer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputForwarding {
    /// The producer receives input through the display server (an OS-managed child view). The
    /// framework does not forward input to it. This is the default.
    #[default]
    Native,
    /// The framework forwards pointer and wheel events that land on the surface (after
    /// framework capture) to the handlers registered for it.
    Framework,
}

/// The platform side of a native or external-GPU surface (PLT-001 R10).
///
/// All geometry is in window-content device pixels. The framework calls the setters from
/// inside [`Window::on_present`](crate::Window::on_present) callbacks only, so the platform
/// makes the changes visible together with the framework buffer of the same frame (on
/// Wayland by caching the subsurface state until the parent commit, on X11 by issuing the
/// requests in one batch flushed immediately before the window's own presentation).
pub trait NativeAttachment {
    /// Moves and resizes the surface. `bounds` is in window-content device pixels;
    /// `parent_origin` is the origin of the surface's parent attachment in the same space
    /// (zero for children of the window), so nested attachments preserve it. On failure
    /// nothing was applied and the framework retries on the next frame.
    fn set_geometry(
        &self,
        bounds: Bounds<DevicePixels>,
        parent_origin: Point<DevicePixels>,
    ) -> Result<()>;

    /// Shows or hides the surface. Hiding removes it from the composition.
    fn set_visible(&self, visible: bool) -> Result<()>;

    /// The opaque platform handle the producer presents to.
    fn layer(&self) -> NativeLayer;

    /// The generation of the handle returned by [`Self::layer`].
    fn generation(&self) -> u64;

    /// Registers the callback invoked when the compositor surface behind this attachment is
    /// recreated, with the composition handles that are valid afterwards. The framework
    /// re-applies the last geometry and visibility on the next frame.
    fn on_recreated(&self, callback: Box<dyn FnMut(NativeComposition)>);

    /// Sets whether the framework forwards input over the surface to its producer.
    fn set_input_forwarding(&self, forwarding: InputForwarding);
}
