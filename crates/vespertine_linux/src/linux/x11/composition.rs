//! The X11 side of scene layering around native child content (PLT-001 R21-R25): the X
//! requests that create, place, stack and shape native child windows and the transparent
//! override-redirect overlay window, and the pure decisions around them.
//!
//! Everything here works on an explicit connection and window ids, so it can run against any X
//! server without a framework window: the window integration that owns the state (renderers,
//! attachments, frame submission) lives in `window::composition`.
//!
//! # Layout of an X11 composed window
//!
//! * Every framework surface is drawn into the window itself, in composition order.
//! * Native producers are child windows of the window (`create_native_child`).
//! * In composited mode the framework content stacked above the native children is also drawn
//!   into a transparent override-redirect overlay window (`create_overlay_window`) whose input
//!   shape is the overlay hitbox union (`OverlayShape`); the opaque coverage of that content is
//!   cut out of the children's bounding and input shapes (`set_cutout_shape`).
//! * Without suitable compositing the occlusion rectangles of everything above a child are cut
//!   out of that child's shapes instead.

use anyhow::{Context as _, Result};
use vespertine::{
    Bounds, CompositionLayout, DevicePixels, Pixels, SurfaceId, X11CompositingFacts,
    overlay_input_shape, require_x11_shape_extension,
};
use x11rb::{
    connection::{Connection, RequestConnection},
    properties::WmHints,
    protocol::{
        shape::{self, ConnectionExt as _},
        xinput::{self, ConnectionExt as _},
        xproto::{self, ConnectionExt as _},
    },
    wrapper::ConnectionExt as _,
};

use super::{XINPUT_ALL_DEVICE_GROUPS, XcbAtoms};

/// Window geometry in X protocol terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

fn saturate_i16(value: i32) -> i16 {
    value.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

fn saturate_u16(value: i32) -> u16 {
    value.clamp(0, u16::MAX as i32) as u16
}

/// The size of `bounds` as X window extents.
pub(crate) fn extent(bounds: Bounds<DevicePixels>) -> (u16, u16) {
    (
        saturate_u16(bounds.size.width.0),
        saturate_u16(bounds.size.height.0),
    )
}

impl Geometry {
    /// The geometry of device-pixel `bounds` placed relative to an X parent at `parent_origin`,
    /// both in window-content coordinates. A window cannot be empty on X11, so a zero extent
    /// becomes one pixel; callers keep such a window unmapped (see [`Geometry::is_degenerate`]).
    pub(crate) fn relative(
        bounds: Bounds<DevicePixels>,
        parent_origin: vespertine::Point<DevicePixels>,
    ) -> Self {
        Geometry {
            x: saturate_i16(bounds.origin.x.0 - parent_origin.x.0),
            y: saturate_i16(bounds.origin.y.0 - parent_origin.y.0),
            width: saturate_u16(bounds.size.width.0).max(1),
            height: saturate_u16(bounds.size.height.0).max(1),
        }
    }

    /// Whether `bounds` has no area, so the window must not be shown.
    pub(crate) fn is_degenerate(bounds: Bounds<DevicePixels>) -> bool {
        bounds.size.width.0 <= 0 || bounds.size.height.0 <= 0
    }
}

/// `rect` as an X rectangle clipped to `[0, size)`, or `None` when nothing remains.
pub(crate) fn clip_rectangle(
    rect: Bounds<DevicePixels>,
    size: (u16, u16),
) -> Option<xproto::Rectangle> {
    let left = rect.origin.x.0.max(0);
    let top = rect.origin.y.0.max(0);
    let right = (rect.origin.x.0.saturating_add(rect.size.width.0)).min(i32::from(size.0));
    let bottom = (rect.origin.y.0.saturating_add(rect.size.height.0)).min(i32::from(size.1));
    (right > left && bottom > top).then(|| xproto::Rectangle {
        x: left as i16,
        y: top as i16,
        width: (right - left) as u16,
        height: (bottom - top) as u16,
    })
}

/// The cutout rectangles (window-content coordinates) in the coordinates of the native child
/// whose window-content bounds are `child`, clipped to the child.
pub(crate) fn cutout_in_child(
    cutout: &[Bounds<DevicePixels>],
    child: Bounds<DevicePixels>,
) -> Vec<xproto::Rectangle> {
    let size = (
        saturate_u16(child.size.width.0),
        saturate_u16(child.size.height.0),
    );
    cutout
        .iter()
        .filter_map(|rect| {
            let local = Bounds {
                origin: vespertine::point(
                    DevicePixels(rect.origin.x.0.saturating_sub(child.origin.x.0)),
                    DevicePixels(rect.origin.y.0.saturating_sub(child.origin.y.0)),
                ),
                size: rect.size,
            };
            clip_rectangle(local, size)
        })
        .collect()
}

/// Whether the X server provides the extension called `name`.
pub(crate) fn extension_present<C: RequestConnection>(connection: &C, name: &'static str) -> bool {
    matches!(connection.extension_information(name), Ok(Some(_)))
}

/// Enabling composition on X11 needs the SHAPE extension (PLT-001 R21).
pub(crate) fn require_shape<C: RequestConnection>(connection: &C) -> Result<()> {
    require_x11_shape_extension(extension_present(connection, shape::X11_EXTENSION_NAME))
        .map_err(anyhow::Error::from)
}

/// Whether the server is an XWayland instance, which always has a compositor in front of it.
pub(crate) fn is_xwayland<C: RequestConnection>(connection: &C) -> bool {
    extension_present(connection, "XWAYLAND")
}

/// The facts [`vespertine::decide_x11_composition_mode`] decides on.
pub(crate) fn compositing_facts<C: RequestConnection>(
    connection: &C,
    argb_visual: bool,
    compositing_manager: impl FnOnce() -> bool,
) -> X11CompositingFacts {
    let xwayland = is_xwayland(connection);
    X11CompositingFacts {
        argb_visual,
        xwayland,
        // The compositing selection only matters on a plain X server.
        compositing_manager: !xwayland && compositing_manager(),
    }
}

/// Creates a native child window of `parent`: opaque, on the screen's default visual, unmapped
/// and not yet placed. The embedder's producer presents into it.
pub(crate) fn create_native_child<C: Connection>(
    connection: &C,
    screen: &xproto::Screen,
    parent: xproto::Window,
) -> Result<xproto::Window> {
    let window = connection.generate_id()?;
    connection
        .create_window(
            screen.root_depth,
            window,
            parent,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            // A depth or visual different from the parent's needs its own border pixel and
            // colormap.
            &xproto::CreateWindowAux::new()
                .border_pixel(screen.black_pixel)
                .colormap(screen.default_colormap),
        )?
        .check()
        .context("X11 CreateWindow for a native child failed")?;
    Ok(window)
}

/// Queues the move and resize of a window. Nothing is flushed: the caller sends one batch right
/// before presenting.
pub(crate) fn place_window<C: Connection>(
    connection: &C,
    window: xproto::Window,
    geometry: Geometry,
) -> Result<()> {
    connection.configure_window(
        window,
        &xproto::ConfigureWindowAux::new()
            .x(i32::from(geometry.x))
            .y(i32::from(geometry.y))
            .width(u32::from(geometry.width))
            .height(u32::from(geometry.height)),
    )?;
    Ok(())
}

/// Queues mapping or unmapping a window.
pub(crate) fn set_mapped<C: Connection>(
    connection: &C,
    window: xproto::Window,
    mapped: bool,
) -> Result<()> {
    if mapped {
        connection.map_window(window)?;
    } else {
        connection.unmap_window(window)?;
    }
    Ok(())
}

/// Queues stacking `windows`, which must be siblings, bottom to top.
pub(crate) fn stack_siblings<C: Connection>(
    connection: &C,
    windows: &[xproto::Window],
) -> Result<()> {
    let mut below: Option<xproto::Window> = None;
    for window in windows {
        let aux = match below {
            // The lowest of all siblings.
            None => xproto::ConfigureWindowAux::new().stack_mode(xproto::StackMode::BELOW),
            Some(sibling) => xproto::ConfigureWindowAux::new()
                .sibling(sibling)
                .stack_mode(xproto::StackMode::ABOVE),
        };
        connection.configure_window(*window, &aux)?;
        below = Some(*window);
    }
    Ok(())
}

/// Cuts `cutout` (rectangles in the window's own coordinates) out of both the bounding and the
/// input shape of a native child of size `size`, so the parent's pixels and input show through.
/// An empty cutout restores the default, unshaped window.
pub(crate) fn set_cutout_shape<C: Connection>(
    connection: &C,
    window: xproto::Window,
    size: (u16, u16),
    cutout: &[xproto::Rectangle],
) -> Result<()> {
    for kind in [shape::SK::BOUNDING, shape::SK::INPUT] {
        if cutout.is_empty() {
            connection.shape_mask(shape::SO::SET, kind, window, 0, 0, x11rb::NONE)?;
            continue;
        }
        connection.shape_rectangles(
            shape::SO::SET,
            kind,
            xproto::ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &[xproto::Rectangle {
                x: 0,
                y: 0,
                width: size.0,
                height: size.1,
            }],
        )?;
        connection.shape_rectangles(
            shape::SO::SUBTRACT,
            kind,
            xproto::ClipOrdering::UNSORTED,
            window,
            0,
            0,
            cutout,
        )?;
    }
    Ok(())
}

/// The shapes of the overlay window: its input shape is the overlay hitbox union (empty
/// outside hitboxes, so pointer events reach the windows below), and native children hosted
/// above the overlay punch holes into both its bounding and input shape so they stay visible
/// and reachable (PLT-001 R11, R14, R22).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OverlayShape {
    pub size: (u16, u16),
    pub input: Vec<xproto::Rectangle>,
    pub holes: Vec<xproto::Rectangle>,
}

impl OverlayShape {
    /// The shape for an overlay window of `size` device pixels: `region` is the overlay's input
    /// region in window-content logical pixels, `holes` the bounds of the natives hosted above
    /// it in window-content device pixels.
    pub(crate) fn new(
        size: (u16, u16),
        region: &[Bounds<Pixels>],
        scale_factor: f32,
        holes: &[Bounds<DevicePixels>],
    ) -> Self {
        OverlayShape {
            size,
            input: overlay_input_shape(region, scale_factor)
                .into_iter()
                .filter_map(|rect| clip_rectangle(rect, size))
                .collect(),
            holes: holes
                .iter()
                .filter_map(|rect| clip_rectangle(*rect, size))
                .collect(),
        }
    }
}

/// Queues the shapes of the overlay window.
pub(crate) fn set_overlay_shape<C: Connection>(
    connection: &C,
    window: xproto::Window,
    shape: &OverlayShape,
) -> Result<()> {
    connection.shape_rectangles(
        shape::SO::SET,
        shape::SK::INPUT,
        xproto::ClipOrdering::UNSORTED,
        window,
        0,
        0,
        &shape.input,
    )?;
    if shape.holes.is_empty() {
        connection.shape_mask(
            shape::SO::SET,
            shape::SK::BOUNDING,
            window,
            0,
            0,
            x11rb::NONE,
        )?;
        return Ok(());
    }
    connection.shape_rectangles(
        shape::SO::SUBTRACT,
        shape::SK::INPUT,
        xproto::ClipOrdering::UNSORTED,
        window,
        0,
        0,
        &shape.holes,
    )?;
    connection.shape_rectangles(
        shape::SO::SET,
        shape::SK::BOUNDING,
        xproto::ClipOrdering::UNSORTED,
        window,
        0,
        0,
        &[xproto::Rectangle {
            x: 0,
            y: 0,
            width: shape.size.0,
            height: shape.size.1,
        }],
    )?;
    connection.shape_rectangles(
        shape::SO::SUBTRACT,
        shape::SK::BOUNDING,
        xproto::ClipOrdering::UNSORTED,
        window,
        0,
        0,
        &shape.holes,
    )?;
    Ok(())
}

/// What the overlay window is created from.
pub(crate) struct OverlayWindowSpec<'a> {
    pub root: xproto::Window,
    /// The parent's ARGB visual and its depth.
    pub visual: xproto::Visualid,
    pub depth: u8,
    /// The framework window the overlay belongs to.
    pub owner: xproto::Window,
    pub geometry: Geometry,
    pub atoms: &'a XcbAtoms,
}

/// The X resources of an overlay window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OverlayWindowIds {
    pub window: xproto::Window,
    pub colormap: xproto::Colormap,
}

/// Creates the transparent overlay window (PLT-001 R22): override-redirect, the parent's ARGB
/// visual with a colormap created for it, transient for the owner, a dropdown-menu type window
/// that does not accept input focus, with pointer motion, button and enter/leave device events
/// selected and an empty input shape until content sets one. The window is not mapped.
pub(crate) fn create_overlay_window<C: Connection>(
    connection: &C,
    spec: &OverlayWindowSpec<'_>,
) -> Result<OverlayWindowIds> {
    let colormap = connection.generate_id()?;
    connection
        .create_colormap(
            xproto::ColormapAlloc::NONE,
            colormap,
            spec.root,
            spec.visual,
        )?
        .check()
        .context("X11 CreateColormap for the overlay window failed")?;
    let window = connection.generate_id()?;
    let created = connection
        .create_window(
            spec.depth,
            window,
            spec.root,
            spec.geometry.x,
            spec.geometry.y,
            spec.geometry.width,
            spec.geometry.height,
            0,
            xproto::WindowClass::INPUT_OUTPUT,
            spec.visual,
            &xproto::CreateWindowAux::new()
                .override_redirect(1)
                // A transparent black background and a window of the parent's visual, which
                // is not the root's, need their own border pixel and colormap.
                .background_pixel(0)
                .border_pixel(0)
                .colormap(colormap),
        )?
        .check()
        .context("X11 CreateWindow for the overlay window failed");
    if let Err(error) = created {
        let _ = connection.free_colormap(colormap);
        return Err(error);
    }
    let result = (|| -> Result<()> {
        connection.change_property32(
            xproto::PropMode::REPLACE,
            window,
            spec.atoms.WM_TRANSIENT_FOR,
            xproto::AtomEnum::WINDOW,
            &[spec.owner],
        )?;
        connection.change_property32(
            xproto::PropMode::REPLACE,
            window,
            spec.atoms._NET_WM_WINDOW_TYPE,
            xproto::AtomEnum::ATOM,
            &[spec.atoms._NET_WM_WINDOW_TYPE_DROPDOWN_MENU],
        )?;
        // The window never takes input focus, so a compositor does not focus it, deactivate
        // the parent (closing framework menus and popovers) or raise a floating parent
        // above it.
        let hints = WmHints {
            input: Some(false),
            ..WmHints::new()
        };
        hints.set(connection, window)?;
        connection.xinput_xi_select_events(
            window,
            &[xinput::EventMask {
                deviceid: XINPUT_ALL_DEVICE_GROUPS,
                mask: vec![
                    xinput::XIEventMask::MOTION
                        | xinput::XIEventMask::BUTTON_PRESS
                        | xinput::XIEventMask::BUTTON_RELEASE
                        | xinput::XIEventMask::ENTER
                        | xinput::XIEventMask::LEAVE,
                ],
            }],
        )?;
        // Empty until set from content: events outside hitboxes pass through.
        connection.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            xproto::ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &[],
        )?;
        connection.flush()?;
        Ok(())
    })();
    if let Err(error) = result {
        destroy_overlay_window(connection, OverlayWindowIds { window, colormap });
        return Err(error.context("setting up the overlay window failed"));
    }
    Ok(OverlayWindowIds { window, colormap })
}

/// Maps the overlay window and restacks it immediately above its owner. Restacking fails when a
/// window manager has reparented the owner into a frame, which is harmless.
pub(crate) fn raise_overlay_window<C: Connection>(
    connection: &C,
    overlay: xproto::Window,
    owner: xproto::Window,
) -> Result<()> {
    connection.map_window(overlay)?;
    connection
        .configure_window(
            overlay,
            &xproto::ConfigureWindowAux::new()
                .sibling(owner)
                .stack_mode(xproto::StackMode::ABOVE),
        )?
        .ignore_error();
    Ok(())
}

/// Destroys the overlay window and its colormap. The caller releases the renderer that presents
/// to the window first.
pub(crate) fn destroy_overlay_window<C: Connection>(connection: &C, ids: OverlayWindowIds) {
    let _ = connection.destroy_window(ids.window);
    let _ = connection.free_colormap(ids.colormap);
    let _ = connection.flush();
}

/// Selects (or, for [`vespertine::InputForwarding::Native`], stops selecting) the pointer
/// events the framework forwards for a native child.
pub(crate) fn select_forwarded_pointer_events<C: Connection>(
    connection: &C,
    window: xproto::Window,
    forward: bool,
) -> Result<()> {
    let mask = if forward {
        vec![
            xinput::XIEventMask::MOTION
                | xinput::XIEventMask::BUTTON_PRESS
                | xinput::XIEventMask::BUTTON_RELEASE,
        ]
    } else {
        Vec::new()
    };
    connection.xinput_xi_select_events(
        window,
        &[xinput::EventMask {
            deviceid: XINPUT_ALL_DEVICE_GROUPS,
            mask,
        }],
    )?;
    Ok(())
}

/// Whether `focus` is a descendant of `owner`: a native child (or a window inside one) holds
/// the keyboard focus, not the framework window itself.
pub(crate) fn focus_inside<C: Connection>(
    connection: &C,
    owner: xproto::Window,
    focus: xproto::Window,
) -> bool {
    let mut window = focus;
    // A window tree is shallow; the bound only guards against a connection that lies.
    for _ in 0..64 {
        if window == owner {
            return window != focus;
        }
        if window == x11rb::NONE {
            return false;
        }
        match connection.query_tree(window).map(|cookie| cookie.reply()) {
            Ok(Ok(tree)) if tree.parent != window => window = tree.parent,
            _ => return false,
        }
    }
    false
}

/// How the framework surfaces split between the window itself and the overlay window in
/// composited mode.
///
/// The window always receives every surface (PLT-001 R21, R23). The overlay window receives the
/// framework surfaces stacked above the topmost visible native child that is not hosted above
/// the overlay: only content above a native child has native pixels to blend over. Visible
/// natives stacked above the overlay surface (hosted in a dialog, R11) cannot be put above an
/// overlay window as child windows, so they punch holes into it instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OverlayPlan {
    /// The framework surfaces drawn into the overlay window, bottom to top.
    pub window_layers: Vec<SurfaceId>,
    /// The visible natives stacked above the overlay surface.
    pub hosted_above: Vec<SurfaceId>,
}

impl OverlayPlan {
    /// Whether an overlay window has anything to be above.
    pub(crate) fn has_window(&self) -> bool {
        !self.window_layers.is_empty()
    }
}

/// Plans the overlay window for `layout` given the natives that are currently shown.
pub(crate) fn plan_overlay(
    layout: &CompositionLayout,
    visible_natives: &[SurfaceId],
) -> OverlayPlan {
    let Some(overlay_position) = layout.position(layout.overlay) else {
        return OverlayPlan::default();
    };
    let positioned =
        |native: &SurfaceId| layout.position(*native).map(|position| (*native, position));
    let top_below = visible_natives
        .iter()
        .filter_map(positioned)
        .filter(|(_, position)| *position < overlay_position)
        .map(|(_, position)| position)
        .max();
    let hosted_above = visible_natives
        .iter()
        .filter_map(positioned)
        .filter(|(_, position)| *position > overlay_position)
        .map(|(native, _)| native)
        .collect();
    let window_layers = match top_below {
        Some(top) => layout
            .entries
            .iter()
            .enumerate()
            .filter(|(position, entry)| {
                *position > top && entry.kind == vespertine::SurfaceKind::Framework
            })
            .map(|(_, entry)| entry.id)
            .collect(),
        None => Vec::new(),
    };
    OverlayPlan {
        window_layers,
        hosted_above,
    }
}

#[cfg(test)]
mod tests;
