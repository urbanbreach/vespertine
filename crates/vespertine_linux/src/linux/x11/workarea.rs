//! The X11 work area (PLT-008 R1): the part of a display that panels and docks leave free.
//!
//! The work area is the `_NET_WORKAREA` of the current desktop intersected with the display
//! bounds. Without the property, or when it describes no usable area, it is the display
//! bounds.

use vespertine::{Bounds, Pixels, point, px, size};
use x11rb::{
    protocol::xproto::{self, AtomEnum, ConnectionExt as _},
    xcb_ffi::XCBConnection,
};

/// The work area from the contents of `_NET_WORKAREA` (x, y, width and height in device
/// pixels for each desktop, in order) and `_NET_CURRENT_DESKTOP`.
pub(crate) fn work_area(
    workarea: &[u32],
    current_desktop: Option<u32>,
    scale_factor: f32,
    display: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let start = current_desktop.unwrap_or(0) as usize * 4;
    let Some(&[x, y, width, height]) = workarea.get(start..start + 4) else {
        return display;
    };
    // Origins are CARDINALs that hold signed positions when monitors lie left of or above
    // the screen origin.
    let area = Bounds::new(
        point(
            px((x as i32) as f32 / scale_factor),
            px((y as i32) as f32 / scale_factor),
        ),
        size(
            px(width as f32 / scale_factor),
            px(height as f32 / scale_factor),
        ),
    );
    let visible = display.intersect(&area);
    if visible.size.width <= px(0.) || visible.size.height <= px(0.) {
        display
    } else {
        visible
    }
}

/// Reads the work area of the screen whose root window is `root`. Every failure to read
/// the properties yields `display`.
pub(crate) fn read_work_area(
    xcb: &XCBConnection,
    root: xproto::Window,
    scale_factor: f32,
    display: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let read = || -> Option<Bounds<Pixels>> {
        let workarea_atom = xcb.intern_atom(false, b"_NET_WORKAREA").ok()?;
        let desktop_atom = xcb.intern_atom(false, b"_NET_CURRENT_DESKTOP").ok()?;
        let workarea_atom = workarea_atom.reply().ok()?.atom;
        let desktop_atom = desktop_atom.reply().ok()?.atom;
        let workarea = xcb
            .get_property(false, root, workarea_atom, AtomEnum::CARDINAL, 0, 4096)
            .ok()?;
        let desktop = xcb
            .get_property(false, root, desktop_atom, AtomEnum::CARDINAL, 0, 1)
            .ok()?;
        let workarea: Vec<u32> = workarea.reply().ok()?.value32()?.collect();
        let desktop = desktop.reply().ok()?.value32()?.next();
        Some(work_area(&workarea, desktop, scale_factor, display))
    };
    read().unwrap_or(display)
}

/// Asks for property change events on the root window, so that changes of the work area
/// and of the current desktop are noticed.
pub(crate) fn watch_root_properties(xcb: &XCBConnection, root: xproto::Window) {
    let result = (|| -> anyhow::Result<()> {
        let current = xcb.get_window_attributes(root)?.reply()?.your_event_mask;
        xcb.change_window_attributes(
            root,
            &xproto::ChangeWindowAttributesAux::new()
                .event_mask(current | xproto::EventMask::PROPERTY_CHANGE),
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        log::warn!("not watching the work area: {error:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(width), px(height)))
    }

    #[test]
    fn work_area_of_the_current_desktop_is_used() {
        let display = bounds(0., 0., 1920., 1080.);
        // Desktop 0 has no panel, desktop 1 has a 40 px panel at the top.
        let property = [0, 0, 1920, 1080, 0, 40, 1920, 1040];
        assert_eq!(
            work_area(&property, Some(1), 1.0, display),
            bounds(0., 40., 1920., 1040.)
        );
        assert_eq!(work_area(&property, Some(0), 1.0, display), display);
        assert_eq!(work_area(&property, None, 1.0, display), display);
    }

    #[test]
    fn work_area_is_intersected_with_the_display_and_scaled() {
        let display = bounds(0., 0., 960., 540.);
        // A 2x scale: 40 px of panel in device pixels is 20 logical pixels.
        let property = [0, 40, 1920, 1040];
        assert_eq!(
            work_area(&property, Some(0), 2.0, display),
            bounds(0., 20., 960., 520.)
        );
        // A work area larger than the display is cut to it.
        let tall = [0, 0, 4000, 4000];
        assert_eq!(work_area(&tall, Some(0), 2.0, display), display);
    }

    #[test]
    fn absent_short_or_empty_properties_give_the_display_bounds() {
        let display = bounds(0., 0., 1920., 1080.);
        assert_eq!(work_area(&[], Some(0), 1.0, display), display);
        assert_eq!(work_area(&[0, 0, 100], Some(0), 1.0, display), display);
        assert_eq!(work_area(&[0, 0, 100, 100], Some(3), 1.0, display), display);
        // Entirely outside the display.
        assert_eq!(
            work_area(&[5000, 5000, 100, 100], Some(0), 1.0, display),
            display
        );
    }
}
