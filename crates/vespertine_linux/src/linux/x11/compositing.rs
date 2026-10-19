//! What the X11 session offers for client-side decorations and window transparency.

use vespertine::{Pixels, Tiling, WindowBackgroundCapabilities, WindowDecorations};

/// The `_GTK_FRAME_EXTENTS` value (left, right, top, bottom) in device pixels for a logical
/// client `inset` at `scale_factor`: the product truncated, and 0 for tiled edges.
pub(crate) fn frame_extents(inset: Pixels, scale_factor: f32, tiling: Tiling) -> [u32; 4] {
    let device_pixels = (f32::from(inset) * scale_factor) as u32;
    let extent = |tiled: bool| if tiled { 0 } else { device_pixels };
    [
        extent(tiling.left),
        extent(tiling.right),
        extent(tiling.top),
        extent(tiling.bottom),
    ]
}

/// The compositing facts of an X11 screen that decide how windows may be decorated and shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct X11Compositing {
    /// A compositing manager owns the compositing selection (`_NET_WM_CM_S<n>`) of the screen or
    /// publishes `_NET_WM_CM_OWNER`.
    pub compositor_present: bool,
    /// The window manager lists `_GTK_FRAME_EXTENTS` in `_NET_SUPPORTED`.
    pub frame_extents_supported: bool,
}

impl X11Compositing {
    /// Whether windows may draw their own decorations.
    pub fn client_decorations_supported(self) -> bool {
        client_decorations_supported(self.compositor_present, self.frame_extents_supported)
    }

    /// Transparency needs a compositing manager; no blur is promised on X11.
    pub fn background_capabilities(self) -> WindowBackgroundCapabilities {
        WindowBackgroundCapabilities {
            transparency: self.compositor_present,
            blur: false,
        }
    }

    /// The decoration mode that results from a request on this screen.
    pub fn effective_decorations(self, requested: WindowDecorations) -> WindowDecorations {
        effective_decorations(
            requested,
            self.compositor_present,
            self.frame_extents_supported,
        )
    }
}

/// Client-side decorations are only effective when a compositing manager owns the screen and the
/// window manager supports `_GTK_FRAME_EXTENTS`.
pub(crate) fn client_decorations_supported(
    compositor_present: bool,
    frame_extents_supported: bool,
) -> bool {
    compositor_present && frame_extents_supported
}

/// The decoration mode a window gets for a `requested` mode: a client-side request silently
/// degrades to server-side (the window manager frame) when client-side decorations are not
/// effective.
pub(crate) fn effective_decorations(
    requested: WindowDecorations,
    compositor_present: bool,
    frame_extents_supported: bool,
) -> WindowDecorations {
    match requested {
        WindowDecorations::Client
            if !client_decorations_supported(compositor_present, frame_extents_supported) =>
        {
            WindowDecorations::Server
        }
        requested => requested,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_request_without_compositor_is_server_side() {
        for frame_extents_supported in [false, true] {
            assert_eq!(
                effective_decorations(WindowDecorations::Client, false, frame_extents_supported),
                WindowDecorations::Server
            );
        }
    }

    #[test]
    fn client_request_without_frame_extents_is_server_side() {
        assert_eq!(
            effective_decorations(WindowDecorations::Client, true, false),
            WindowDecorations::Server
        );
    }

    #[test]
    fn client_request_with_compositor_and_frame_extents_is_client_side() {
        assert_eq!(
            effective_decorations(WindowDecorations::Client, true, true),
            WindowDecorations::Client
        );
    }

    #[test]
    fn server_request_is_always_server_side() {
        for compositor_present in [false, true] {
            for frame_extents_supported in [false, true] {
                assert_eq!(
                    effective_decorations(
                        WindowDecorations::Server,
                        compositor_present,
                        frame_extents_supported
                    ),
                    WindowDecorations::Server
                );
            }
        }
    }

    #[test]
    fn transparency_needs_a_compositor_and_blur_is_never_offered() {
        let without = X11Compositing {
            compositor_present: false,
            frame_extents_supported: true,
        };
        let with = X11Compositing {
            compositor_present: true,
            frame_extents_supported: true,
        };
        assert_eq!(
            without.background_capabilities(),
            WindowBackgroundCapabilities {
                transparency: false,
                blur: false
            }
        );
        assert_eq!(
            with.background_capabilities(),
            WindowBackgroundCapabilities {
                transparency: true,
                blur: false
            }
        );
        assert_eq!(
            without.effective_decorations(WindowDecorations::Client),
            WindowDecorations::Server
        );
    }
}

#[cfg(test)]
mod frame_extents_tests {
    use super::*;
    use vespertine::px;

    #[test]
    fn extents_are_device_pixels_truncated() {
        assert_eq!(
            frame_extents(px(10.0), 1.25, Tiling::default()),
            [12, 12, 12, 12]
        );
        assert_eq!(
            frame_extents(px(10.0), 2.0, Tiling::default()),
            [20, 20, 20, 20]
        );
    }

    #[test]
    fn tiled_edges_have_no_extent() {
        let tiling = Tiling {
            top: true,
            left: false,
            right: false,
            bottom: true,
        };
        assert_eq!(frame_extents(px(10.0), 1.0, tiling), [10, 10, 0, 0]);
        assert_eq!(frame_extents(px(10.0), 1.0, Tiling::tiled()), [0, 0, 0, 0]);
    }
}
