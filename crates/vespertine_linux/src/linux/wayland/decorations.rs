//! Decoration mode negotiation rules for Wayland (xdg-decoration).

use vespertine::WindowDecorations;
use wayland_client::WEnum;
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode;

/// The decoration mode a window has right after the application requested `requested`, before
/// any compositor answer arrives.
///
/// When the compositor does not offer xdg-decoration at all there is nobody to draw a frame, so
/// the window is client-decorated regardless of the request.
pub(crate) fn mode_after_request(
    requested: WindowDecorations,
    decoration_protocol_available: bool,
) -> WindowDecorations {
    if decoration_protocol_available {
        requested
    } else {
        WindowDecorations::Client
    }
}

/// The decoration mode the compositor's configure answer establishes. The answer is
/// authoritative: it may differ from what was requested, in either direction.
pub(crate) fn mode_from_configure(mode: WEnum<Mode>) -> Option<WindowDecorations> {
    match mode {
        WEnum::Value(Mode::ServerSide) => Some(WindowDecorations::Server),
        WEnum::Value(Mode::ClientSide) => Some(WindowDecorations::Client),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn without_the_protocol_every_window_is_client_decorated() {
        assert_eq!(
            mode_after_request(WindowDecorations::Server, false),
            WindowDecorations::Client
        );
        assert_eq!(
            mode_after_request(WindowDecorations::Client, false),
            WindowDecorations::Client
        );
    }

    #[test]
    fn with_the_protocol_the_request_stands_until_the_compositor_answers() {
        assert_eq!(
            mode_after_request(WindowDecorations::Server, true),
            WindowDecorations::Server
        );
        assert_eq!(
            mode_after_request(WindowDecorations::Client, true),
            WindowDecorations::Client
        );
    }

    #[test]
    fn the_compositor_answer_overrides_the_request() {
        // A server-side request answered with client-side, and the other way around.
        assert_eq!(
            mode_from_configure(WEnum::Value(Mode::ClientSide)),
            Some(WindowDecorations::Client)
        );
        assert_eq!(
            mode_from_configure(WEnum::Value(Mode::ServerSide)),
            Some(WindowDecorations::Server)
        );
        assert_eq!(mode_from_configure(WEnum::Unknown(7)), None);
    }
}
