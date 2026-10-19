//! `_NET_WM_MOVERESIZE` request construction.

use vespertine::{MouseButton, NavigationDirection, ResizeEdge};

/// `_NET_WM_MOVERESIZE` direction code that starts a window move.
pub(crate) const MOVERESIZE_MOVE: u32 = 8;

/// The source indication of a request from an ordinary application (as opposed to a pager).
const SOURCE_INDICATION_APPLICATION: u32 = 1;

/// The `_NET_WM_MOVERESIZE` direction code for a resize from `edge`.
pub(crate) fn resize_edge_to_moveresize(edge: ResizeEdge) -> u32 {
    match edge {
        ResizeEdge::TopLeft => 0,
        ResizeEdge::Top => 1,
        ResizeEdge::TopRight => 2,
        ResizeEdge::Right => 3,
        ResizeEdge::BottomRight => 4,
        ResizeEdge::Bottom => 5,
        ResizeEdge::BottomLeft => 6,
        ResizeEdge::Left => 7,
    }
}

/// The X11 core button number of a pointer button.
pub(crate) fn x_button_number(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::Navigate(NavigationDirection::Back) => 8,
        MouseButton::Navigate(NavigationDirection::Forward) => 9,
    }
}

/// The 32-bit data words of a `_NET_WM_MOVERESIZE` client message: root-relative pointer
/// position, direction code, the pressed button and source indication 1.
pub(crate) fn moveresize_data(root_x: i32, root_y: i32, direction: u32, button: u32) -> [u32; 5] {
    [
        root_x as u32,
        root_y as u32,
        direction,
        button,
        SOURCE_INDICATION_APPLICATION,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_codes_follow_the_ewmh_numbering() {
        let expected = [
            (ResizeEdge::TopLeft, 0),
            (ResizeEdge::Top, 1),
            (ResizeEdge::TopRight, 2),
            (ResizeEdge::Right, 3),
            (ResizeEdge::BottomRight, 4),
            (ResizeEdge::Bottom, 5),
            (ResizeEdge::BottomLeft, 6),
            (ResizeEdge::Left, 7),
        ];
        for (edge, code) in expected {
            assert_eq!(resize_edge_to_moveresize(edge), code, "{edge:?}");
        }
        assert_eq!(MOVERESIZE_MOVE, 8);
    }

    #[test]
    fn message_carries_pointer_button_and_application_source() {
        let data = moveresize_data(120, -4, MOVERESIZE_MOVE, x_button_number(MouseButton::Left));
        assert_eq!(data, [120, -4i32 as u32, 8, 1, 1]);
        assert_eq!(x_button_number(MouseButton::Middle), 2);
        assert_eq!(x_button_number(MouseButton::Right), 3);
    }
}
