//! Client-side window chrome for Linux (Wayland and X11).
//!
//! Applications that draw their own titlebar need four things from the window system, all of
//! which live here:
//!
//! * pure geometry helpers ([`WindowChromeGeometry`]) that decide which resize edge lies under
//!   a point, which corners are rounded and how much invisible shadow inset applies to each edge,
//! * the background-effect capability model ([`WindowBackgroundCapabilities`]) with the opaque
//!   fallback rule: a window that asks for an effect the desktop cannot provide is rendered
//!   fully opaque, with square corners and no shadow inset,
//! * the pointer-press contract of native move/resize: such requests are only honoured while a
//!   pointer button is held down, and once the compositor takes over the application state is
//!   reset as if the button had been released ([`Window::start_window_move`]),
//! * reusable chrome elements: [`window_resize_handles`], the [`WindowChromeElement`] drag
//!   region marker and the [`CaptionControls`] component.

use crate::{
    AnyElement, App, Background, Corners, CursorStyle, Decorations, DispatchPhase, Edges, Empty,
    Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PathBuilder, Pixels, PlatformInput, Point, RenderOnce, ResizeEdge,
    SharedString, Size, StatefulInteractiveElement, Styled, Tiling, Window,
    WindowBackgroundAppearance, WindowButton, WindowButtonLayout, WindowControlArea,
    WindowControls, canvas, div, hsla, point, px, rgb, size,
};
use std::{cell::Cell, rc::Rc};

/// Thickness, in logical pixels, of the client-side resize band along each outer window edge.
pub const RESIZE_BAND_THICKNESS: Pixels = px(6.0);

/// Length, in logical pixels, of the corner region along each edge. The corner region is this
/// long along the edge and [`RESIZE_BAND_THICKNESS`] deep.
pub const RESIZE_CORNER_LENGTH: Pixels = px(14.0);

/// Corner radius, in logical pixels, of a floating (untiled, unmaximized) client-decorated window.
pub const FLOATING_CORNER_RADIUS: Pixels = px(10.0);

/// Default width of a caption control hit region.
pub const CAPTION_CONTROL_WIDTH: Pixels = px(40.0);

/// Default height of a caption control hit region and of the default titlebar.
pub const CAPTION_CONTROL_HEIGHT: Pixels = px(38.0);

/// Edge length of the default caption control glyphs.
pub const CAPTION_GLYPH_SIZE: Pixels = px(10.0);

/// Window state that determines the shape of client-side chrome.
///
/// All methods are pure functions of this value, so they can be tested without a display server.
/// Coordinates are logical pixels measured inside the window's *outer* bounds, which include the
/// client inset when the application draws an invisible shadow margin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowChromeGeometry {
    /// Outer size of the window.
    pub size: Size<Pixels>,
    /// Which edges are tiled against other windows or the screen.
    pub tiling: Tiling,
    /// Whether the window is maximized.
    pub maximized: bool,
    /// Whether the window is fullscreen.
    pub fullscreen: bool,
    /// Whether the user may resize the window.
    pub resizable: bool,
}

impl WindowChromeGeometry {
    /// Whether the window floats freely: neither maximized nor fullscreen.
    pub fn is_floating(&self) -> bool {
        !self.maximized && !self.fullscreen
    }

    /// Returns the resize edge under `point`, or `None` when the point is outside every resize
    /// band, or the band it lies in does not belong to the client.
    ///
    /// A point is in the band of an edge when its distance to that outer edge is strictly less
    /// than [`RESIZE_BAND_THICKNESS`]. A band point whose position along the edge is strictly
    /// less than [`RESIZE_CORNER_LENGTH`] from a corner resolves to the diagonal of that corner,
    /// provided both adjacent edges are free. When only one adjacent edge is free, that edge's
    /// band applies along its full length including the corner span.
    pub fn resize_edge_at(&self, point: Point<Pixels>) -> Option<ResizeEdge> {
        let (band, corner) = (RESIZE_BAND_THICKNESS, RESIZE_CORNER_LENGTH);
        let (x, y) = (point.x, point.y);
        let (right_distance, bottom_distance) = (self.size.width - x, self.size.height - y);
        let outside =
            x < px(0.0) || y < px(0.0) || right_distance < px(0.0) || bottom_distance < px(0.0);
        if outside {
            return None;
        }

        let in_left = x < band;
        let in_right = right_distance < band;
        let in_top = y < band;
        let in_bottom = bottom_distance < band;

        // An edge is free when the window is floating, resizable and the edge is not tiled.
        let client_owned = self.is_floating() && self.resizable;
        let (top, bottom, left, right) = (
            client_owned && !self.tiling.top,
            client_owned && !self.tiling.bottom,
            client_owned && !self.tiling.left,
            client_owned && !self.tiling.right,
        );

        // Distances from the point to each side along the axis perpendicular to that side;
        // the corner span is measured along the other axis.
        let near_left = x < corner;
        let near_right = right_distance < corner;
        let near_top = y < corner;
        let near_bottom = bottom_distance < corner;

        let corner_hit =
            |vertical_band: bool, horizontal_band: bool, near_h: bool, near_v: bool| {
                (vertical_band && near_v) || (horizontal_band && near_h)
            };

        if top && left && corner_hit(in_left, in_top, near_left, near_top) {
            return Some(ResizeEdge::TopLeft);
        }
        if top && right && corner_hit(in_right, in_top, near_right, near_top) {
            return Some(ResizeEdge::TopRight);
        }
        if bottom && left && corner_hit(in_left, in_bottom, near_left, near_bottom) {
            return Some(ResizeEdge::BottomLeft);
        }
        if bottom && right && corner_hit(in_right, in_bottom, near_right, near_bottom) {
            return Some(ResizeEdge::BottomRight);
        }

        if top && in_top {
            Some(ResizeEdge::Top)
        } else if bottom && in_bottom {
            Some(ResizeEdge::Bottom)
        } else if left && in_left {
            Some(ResizeEdge::Left)
        } else if right && in_right {
            Some(ResizeEdge::Right)
        } else {
            None
        }
    }

    /// Radius of each outer corner of a client-decorated window.
    ///
    /// A corner is rounded with [`FLOATING_CORNER_RADIUS`] only when the window is floating and
    /// both edges adjacent to the corner are untiled. Every other corner is square.
    pub fn corner_radii(&self) -> Corners<Pixels> {
        let round = |a: bool, b: bool| {
            if self.is_floating() && !a && !b {
                FLOATING_CORNER_RADIUS
            } else {
                px(0.0)
            }
        };
        Corners {
            top_left: round(self.tiling.top, self.tiling.left),
            top_right: round(self.tiling.top, self.tiling.right),
            bottom_right: round(self.tiling.bottom, self.tiling.right),
            bottom_left: round(self.tiling.bottom, self.tiling.left),
        }
    }

    /// The invisible shadow margin on each edge for a requested `inset`: zero on tiled edges and
    /// on every edge of a maximized or fullscreen window, `inset` elsewhere.
    pub fn client_inset(&self, inset: Pixels) -> Edges<Pixels> {
        let edge = |tiled: bool| {
            if tiled || !self.is_floating() {
                px(0.0)
            } else {
                inset
            }
        };
        Edges {
            top: edge(self.tiling.top),
            right: edge(self.tiling.right),
            bottom: edge(self.tiling.bottom),
            left: edge(self.tiling.left),
        }
    }
}

impl ResizeEdge {
    /// The cursor shown while the pointer is over this resize edge.
    pub fn cursor_style(self) -> CursorStyle {
        match self {
            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
            ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        }
    }
}

/// Which window background effects the desktop can provide for a window.
///
/// Reported by the platform window; platforms that do not implement the query report
/// [`WindowBackgroundCapabilities::UNSUPPORTED`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowBackgroundCapabilities {
    /// Whether windows can have transparent pixels.
    ///
    /// Wayland: always. X11: only while a compositing manager owns the screen.
    pub transparency: bool,
    /// Whether the desktop can blur the content behind a window.
    ///
    /// Wayland: only when the compositor offers the KDE blur protocol. X11: never promised.
    pub blur: bool,
}

impl WindowBackgroundCapabilities {
    /// Neither transparency nor blur is available.
    pub const UNSUPPORTED: Self = Self {
        transparency: false,
        blur: false,
    };

    /// Both transparency and blur are available.
    pub const ALL: Self = Self {
        transparency: true,
        blur: true,
    };

    /// The background appearance a window actually gets when `requested` is asked for:
    /// transparency without transparency support, and blur without blur support, both fall back
    /// to [`WindowBackgroundAppearance::Opaque`].
    pub fn effective(&self, requested: WindowBackgroundAppearance) -> WindowBackgroundAppearance {
        match requested {
            WindowBackgroundAppearance::Opaque => WindowBackgroundAppearance::Opaque,
            WindowBackgroundAppearance::Transparent if self.transparency => requested,
            WindowBackgroundAppearance::Transparent => WindowBackgroundAppearance::Opaque,
            other if self.blur => other,
            _ => WindowBackgroundAppearance::Opaque,
        }
    }

    /// Whether the window must render as a plain opaque window: transparency is unavailable, or
    /// `requested` could not be honoured. An opaque window has no transparent pixels, square
    /// corners and zero shadow inset, and uses the theme's opaque window background.
    pub fn requires_opaque_fallback(&self, requested: WindowBackgroundAppearance) -> bool {
        !self.transparency || self.effective(requested) != requested
    }
}

/// The part of a titlebar a [`CaptionControls`] group is placed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionControlsSide {
    /// The left-hand group of the button layout.
    Left,
    /// The right-hand group of the button layout.
    Right,
}

/// The layout used when the desktop does not provide one: nothing on the left, then minimize,
/// maximize and close on the right.
pub fn default_caption_button_layout() -> WindowButtonLayout {
    WindowButtonLayout {
        left: [None; crate::MAX_BUTTONS_PER_SIDE],
        right: [
            Some(WindowButton::Minimize),
            Some(WindowButton::Maximize),
            Some(WindowButton::Close),
        ],
    }
}

/// The caption buttons to draw on one side of the titlebar, in order.
///
/// Buttons the platform does not support are omitted, minimize is omitted for a window that is
/// not minimizable and maximize for a window that is not resizable. Close is always present.
pub fn visible_caption_buttons(
    layout: &WindowButtonLayout,
    side: CaptionControlsSide,
    controls: WindowControls,
    resizable: bool,
    minimizable: bool,
) -> Vec<WindowButton> {
    let buttons = match side {
        CaptionControlsSide::Left => &layout.left,
        CaptionControlsSide::Right => &layout.right,
    };
    buttons
        .iter()
        .flatten()
        .copied()
        .filter(|button| match button {
            WindowButton::Minimize => controls.minimize && minimizable,
            WindowButton::Maximize => controls.maximize && resizable,
            WindowButton::Close => true,
        })
        .collect()
}

/// A change in these values means the application's chrome must be redrawn.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ChromeSnapshot {
    decorations: Decorations,
    controls: WindowControls,
    maximized: bool,
    fullscreen: bool,
    background: WindowBackgroundAppearance,
    capabilities: WindowBackgroundCapabilities,
}

/// Per-window chrome bookkeeping owned by [`Window`].
#[derive(Default)]
pub(crate) struct ChromeWindowState {
    snapshot: Option<ChromeSnapshot>,
    /// The pointer button currently held down in this window, if any.
    pressed_pointer: Option<MouseButton>,
    /// Set when a native move/resize/menu began and the pressed state must be released.
    release_pending: Cell<bool>,
}

impl ChromeWindowState {
    /// Records pointer button transitions as they are dispatched.
    pub(crate) fn note_input(&mut self, input: &PlatformInput) {
        match input {
            PlatformInput::MouseDown(event) => self.pressed_pointer = Some(event.button),
            PlatformInput::MouseUp(event) if self.pressed_pointer == Some(event.button) => {
                self.pressed_pointer = None;
            }
            _ => {}
        }
    }
}

/// Position used for the synthetic release: far outside any window, so no element counts it as
/// hovered and none treats it as the second half of a click.
const OFF_WINDOW: Point<Pixels> = Point {
    x: px(-1.0e6),
    y: px(-1.0e6),
};

impl Window {
    /// The chrome-relevant state of this window as a pure value.
    ///
    /// Tiling comes from [`Window::window_decorations`]; a server-decorated window reports no
    /// tiling because the compositor owns its frame.
    pub fn chrome_geometry(&self) -> WindowChromeGeometry {
        let tiling = match self.window_decorations() {
            Decorations::Client { tiling } => tiling,
            Decorations::Server => Tiling::default(),
        };
        WindowChromeGeometry {
            size: self.viewport_size(),
            tiling,
            maximized: self.is_maximized(),
            fullscreen: self.is_fullscreen(),
            resizable: self.is_resizable(),
        }
    }

    /// Whether the application must draw this window's decorations: true exactly when
    /// [`Window::window_decorations`] reports [`Decorations::Client`].
    pub fn is_client_decorated(&self) -> bool {
        matches!(self.window_decorations(), Decorations::Client { .. })
    }

    /// The resize edge under `point` (window coordinates), or `None`.
    ///
    /// Always `None` for server-decorated windows, whose edges belong to the compositor.
    pub fn resize_edge_at(&self, point: Point<Pixels>) -> Option<ResizeEdge> {
        if !self.is_client_decorated() {
            return None;
        }
        self.chrome_geometry().resize_edge_at(point)
    }

    /// The radius of each outer window corner.
    ///
    /// Server-decorated windows draw no rounding of their own, and a window that fell back to
    /// opaque (see [`Window::is_opaque_fallback`]) has square corners.
    pub fn window_corner_radii(&self) -> Corners<Pixels> {
        if !self.is_client_decorated() || self.is_opaque_fallback() {
            return Corners::all(px(0.0));
        }
        self.chrome_geometry().corner_radii()
    }

    /// The invisible shadow margin currently in effect on each edge: the value passed to
    /// [`Window::set_client_inset`], zeroed on tiled edges, for maximized and fullscreen windows,
    /// for server-decorated windows and for windows that fell back to opaque.
    pub fn effective_client_inset(&self) -> Edges<Pixels> {
        let zero = Edges::all(px(0.0));
        if !self.is_client_decorated() || self.is_opaque_fallback() {
            return zero;
        }
        match self.client_inset() {
            Some(inset) => self.chrome_geometry().client_inset(inset),
            None => zero,
        }
    }

    /// Which background effects (transparency, desktop blur) the platform provides.
    pub fn background_capabilities(&self) -> WindowBackgroundCapabilities {
        self.platform_window.background_capabilities()
    }

    /// The background appearance requested for this window.
    pub fn requested_background_appearance(&self) -> WindowBackgroundAppearance {
        self.platform_window.background_appearance()
    }

    /// The background appearance the window actually has, after applying the opaque fallback to
    /// the requested appearance and the platform's capabilities.
    pub fn effective_background_appearance(&self) -> WindowBackgroundAppearance {
        self.background_capabilities()
            .effective(self.requested_background_appearance())
    }

    /// Whether the window must render as a plain opaque window, because transparency is
    /// unavailable or the requested effect could not be provided.
    pub fn is_opaque_fallback(&self) -> bool {
        self.background_capabilities()
            .requires_opaque_fallback(self.requested_background_appearance())
    }

    /// Picks the window background color: `preferred` normally, `opaque` (the theme's opaque
    /// window background) when the window fell back to opaque.
    pub fn resolve_window_background(&self, preferred: Hsla, opaque: Hsla) -> Hsla {
        if self.is_opaque_fallback() {
            opaque
        } else {
            preferred
        }
    }

    /// The pointer button currently held down in this window, if any. Cleared when a native
    /// move or resize begins, since the compositor then owns the pointer.
    pub fn pressed_pointer_button(&self) -> Option<MouseButton> {
        self.chrome.pressed_pointer
    }

    /// Sends the inset that is effective for the current background fallback state to the platform.
    pub(crate) fn push_client_inset(&self) {
        if let Some(inset) = self.client_inset() {
            let inset = if self.is_opaque_fallback() {
                px(0.0)
            } else {
                inset
            };
            self.platform_window.set_client_inset(inset);
        }
    }

    /// Whether a native move or resize may begin now. They need a held pointer button; when one
    /// is held, the pressed state is released once the current event finishes dispatching.
    pub(crate) fn begin_native_pointer_operation(&self) -> bool {
        if self.chrome.pressed_pointer.is_none() {
            return false;
        }
        self.chrome.release_pending.set(true);
        true
    }

    /// Marks the pressed pointer state for release if a button is held, without requiring one.
    pub(crate) fn release_pointer_for_native_menu(&self) {
        if self.chrome.pressed_pointer.is_some() {
            self.chrome.release_pending.set(true);
        }
    }

    /// Delivers the release the compositor will never send: clears the pressed button, any
    /// pointer capture and any active drag, and lets every element drop its pressed state
    /// without firing click handlers.
    pub(crate) fn flush_pointer_release(&mut self, cx: &mut App) {
        if !self.chrome.release_pending.take() {
            return;
        }
        let Some(button) = self.chrome.pressed_pointer else {
            return;
        };
        let propagate = cx.propagate_event;
        self.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position: OFF_WINDOW,
                modifiers: self.modifiers(),
                click_count: 1,
            }),
            cx,
        );
        self.chrome.pressed_pointer = None;
        cx.propagate_event = propagate;
    }

    /// Redraws the window when decoration mode, tiling, maximized, fullscreen, supported
    /// controls or background effects changed since the last frame request, so chrome never
    /// lags the compositor.
    pub(crate) fn sync_chrome_state(&mut self) {
        let snapshot = ChromeSnapshot {
            decorations: self.window_decorations(),
            controls: self.window_controls(),
            maximized: self.is_maximized(),
            fullscreen: self.is_fullscreen(),
            background: self.requested_background_appearance(),
            capabilities: self.background_capabilities(),
        };
        if let Some(previous) = self.chrome.snapshot.replace(snapshot)
            && previous != snapshot
        {
            self.refresh();
        }
    }

    /// Whether the pointer is over a window-drag region and no interactive element lies above it.
    fn pointer_is_over_drag_region(&self) -> bool {
        let Some(top) = self.mouse_hit_test.ids.first() else {
            return false;
        };
        self.rendered_frame
            .window_control_hitboxes
            .iter()
            .any(|(area, hitbox)| *area == WindowControlArea::Drag && hitbox.id == *top)
    }
}

/// Elements that can mark themselves as the draggable region of a custom titlebar.
pub trait WindowChromeElement: InteractiveElement + Sized {
    /// Makes this element a window drag region.
    ///
    /// * A primary press starts a native window move, unless the press landed on a caption
    ///   control or another interactive element above the region (an element with its own hitbox,
    ///   such as a button or a hover-styled child).
    /// * A double click toggles maximize/restore when the window is resizable and the platform
    ///   supports maximizing.
    /// * A secondary press opens the native window menu at the pointer when the platform
    ///   supports it.
    ///
    /// The element is also flagged as [`WindowControlArea::Drag`] for platforms with native hit
    /// testing.
    fn window_drag_region(self) -> Self {
        self.window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, |event, window, _cx| {
                if !window.pointer_is_over_drag_region() {
                    return;
                }
                if event.click_count == 2 {
                    if window.is_resizable() && window.window_controls().maximize {
                        window.zoom_window();
                    }
                } else {
                    window.start_window_move();
                }
            })
            .on_mouse_down(MouseButton::Right, |event, window, _cx| {
                if window.pointer_is_over_drag_region() && window.window_controls().window_menu {
                    window.show_window_menu(event.position);
                }
            })
    }
}

impl<E: InteractiveElement + Sized> WindowChromeElement for E {}

/// An invisible element that provides the client-side resize hit areas of a client-decorated
/// window: it shows the matching resize cursor and starts a native resize on a primary press.
///
/// Place it once, anywhere in the window's root element (it paints nothing and is sized to fill
/// its parent). Presses inside a resize band are handled before child hit testing, so children
/// cannot swallow edge presses. It does nothing for server-decorated, maximized, fullscreen or
/// non-resizable windows.
pub fn window_resize_handles() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        |_, _, window, _| {
            let current = window.resize_edge_at(window.mouse_position());
            if let Some(edge) = current {
                window.set_window_cursor_style(edge.cursor_style());
            }
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, _cx| {
                if phase == DispatchPhase::Capture
                    && window.resize_edge_at(event.position) != current
                {
                    window.refresh();
                }
            });
            window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture
                    && event.button == MouseButton::Left
                    && let Some(edge) = window.resize_edge_at(event.position)
                {
                    cx.stop_propagation();
                    window.start_window_resize(edge);
                }
            });
        },
    )
    .absolute()
    .size_full()
}

/// Draws a custom glyph for a caption control.
pub type CaptionIcon = Rc<dyn Fn(CaptionGlyphState) -> AnyElement>;

/// What a [`CaptionIcon`] needs to draw its glyph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptionGlyphState {
    /// The glyph color for the control's current hover state.
    pub color: Hsla,
    /// Whether the window is maximized (the maximize control then shows a restore glyph).
    pub maximized: bool,
}

/// Per-control overrides of a [`CaptionControlsStyle`]. Unset fields use the shared style.
#[derive(Clone, Default)]
pub struct CaptionControlOverride {
    /// Replaces the default glyph.
    pub icon: Option<CaptionIcon>,
    /// Replaces the hit region size.
    pub size: Option<Size<Pixels>>,
    /// Replaces the glyph color.
    pub glyph_color: Option<Hsla>,
    /// Replaces the hover background overlay.
    pub hover_background: Option<Hsla>,
    /// Replaces the pressed background overlay.
    pub active_background: Option<Hsla>,
}

/// Colors and sizes of [`CaptionControls`].
///
/// The fields mirror the theme tokens the controls are drawn from: glyph color from the primary
/// text token, `surface` from a surface token, the hover and pressed overlays from the subtle
/// hover and active tokens and the close hover from the destructive token. Until a theme drives
/// them, the defaults are a neutral light palette; applications and theme code assign the
/// fields from their own tokens.
#[derive(Clone)]
pub struct CaptionControlsStyle {
    /// Hit region size of each control; 40 x 38 by default.
    pub control_size: Size<Pixels>,
    /// The titlebar surface the controls sit on.
    pub surface: Hsla,
    /// Glyph color (primary text token).
    pub glyph_color: Hsla,
    /// Overlay blended over [`Self::surface`] while a control is hovered (subtle hover token).
    pub hover_background: Hsla,
    /// Overlay blended over [`Self::surface`] while a control is pressed (subtle active token).
    pub active_background: Hsla,
    /// Background of the close control while hovered (destructive token).
    pub close_hover_background: Hsla,
    /// Background of the close control while pressed.
    pub close_active_background: Hsla,
    /// Glyph color of the close control while hovered or pressed.
    pub close_hover_glyph_color: Hsla,
    /// Overrides for the minimize control.
    pub minimize: CaptionControlOverride,
    /// Overrides for the maximize/restore control.
    pub maximize: CaptionControlOverride,
    /// Overrides for the close control.
    pub close: CaptionControlOverride,
}

impl Default for CaptionControlsStyle {
    fn default() -> Self {
        Self {
            control_size: size(CAPTION_CONTROL_WIDTH, CAPTION_CONTROL_HEIGHT),
            surface: rgb(0xf3f3f3).into(),
            glyph_color: rgb(0x1f1f1f).into(),
            hover_background: hsla(0.0, 0.0, 0.0, 0.08),
            active_background: hsla(0.0, 0.0, 0.0, 0.14),
            close_hover_background: rgb(0xc42b1c).into(),
            close_active_background: rgb(0xa32316).into(),
            close_hover_glyph_color: rgb(0xffffff).into(),
            minimize: CaptionControlOverride::default(),
            maximize: CaptionControlOverride::default(),
            close: CaptionControlOverride::default(),
        }
    }
}

/// The default caption controls: minimize, maximize/restore and close, laid out in the order of
/// the desktop's button layout.
///
/// Renders nothing unless the window reports client-side decorations. Place one group per
/// titlebar side; each draws the buttons the desktop layout assigns to its side, so together the
/// two groups draw each control exactly once. Controls the platform does not support, minimize in
/// a non-minimizable window and maximize in a non-resizable window are omitted. Each control is a
/// 40 x 38 hit region activated by a primary click whose press and release both land inside it;
/// close requests the window to close through [`Window::request_close`].
#[derive(IntoElement)]
pub struct CaptionControls {
    side: CaptionControlsSide,
    style: CaptionControlsStyle,
}

impl CaptionControls {
    /// Caption controls for one side of the titlebar.
    pub fn new(side: CaptionControlsSide) -> Self {
        Self {
            side,
            style: CaptionControlsStyle::default(),
        }
    }

    /// Replaces the style.
    pub fn style(mut self, style: CaptionControlsStyle) -> Self {
        self.style = style;
        self
    }
}

impl RenderOnce for CaptionControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !window.is_client_decorated() {
            return Empty.into_any_element();
        }
        let layout = cx
            .button_layout()
            .unwrap_or_else(default_caption_button_layout);
        let buttons = visible_caption_buttons(
            &layout,
            self.side,
            window.window_controls(),
            window.is_resizable(),
            window.is_minimizable(),
        );
        if buttons.is_empty() {
            return Empty.into_any_element();
        }
        let maximized = window.is_maximized();
        let style = self.style;
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(style.control_size.height)
            .bg(style.surface)
            .children(
                buttons
                    .into_iter()
                    .map(|button| caption_control(button, &style, maximized)),
            )
            .into_any_element()
    }
}

/// The accessible name of a caption control.
pub fn caption_control_name(button: WindowButton, maximized: bool) -> &'static str {
    match button {
        WindowButton::Minimize => "Minimize",
        WindowButton::Maximize if maximized => "Restore",
        WindowButton::Maximize => "Maximize",
        WindowButton::Close => "Close",
    }
}

fn caption_control(
    button: WindowButton,
    style: &CaptionControlsStyle,
    maximized: bool,
) -> AnyElement {
    let (over, area, id) = match button {
        WindowButton::Minimize => (&style.minimize, WindowControlArea::Min, "caption-minimize"),
        WindowButton::Maximize => (&style.maximize, WindowControlArea::Max, "caption-maximize"),
        WindowButton::Close => (&style.close, WindowControlArea::Close, "caption-close"),
    };
    let is_close = button == WindowButton::Close;
    let control_size = over.size.unwrap_or(style.control_size);
    let glyph_color = over.glyph_color.unwrap_or(style.glyph_color);
    let hover_glyph = if is_close {
        style.close_hover_glyph_color
    } else {
        glyph_color
    };
    let (hover_bg, active_bg) = if is_close {
        (style.close_hover_background, style.close_active_background)
    } else {
        (
            style
                .surface
                .blend(over.hover_background.unwrap_or(style.hover_background)),
            style
                .surface
                .blend(over.active_background.unwrap_or(style.active_background)),
        )
    };

    let name: SharedString = caption_control_name(button, maximized).into();
    div()
        .id(id)
        .role(accesskit::Role::Button)
        .aria_label(name)
        .flex()
        .items_center()
        .justify_center()
        .w(control_size.width)
        .h(control_size.height)
        .flex_none()
        .bg(style.surface)
        .window_control_area(area)
        .hover(move |s| s.bg(hover_bg))
        .active(move |s| s.bg(active_bg))
        // Presses on a control must not reach the drag region behind it.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| match button {
            WindowButton::Minimize => window.minimize_window(),
            WindowButton::Maximize => window.zoom_window(),
            WindowButton::Close => {
                window.request_close(cx);
            }
        })
        .child(caption_glyph(
            button,
            over.icon.clone(),
            control_size,
            glyph_color,
            hover_glyph,
            maximized,
        ))
        .into_any_element()
}

/// The glyph of a caption control: the override icon, or the default drawing.
fn caption_glyph(
    button: WindowButton,
    icon: Option<CaptionIcon>,
    control_size: Size<Pixels>,
    color: Hsla,
    hover_color: Hsla,
    maximized: bool,
) -> AnyElement {
    if let Some(icon) = icon {
        return icon(CaptionGlyphState { color, maximized });
    }
    let glyph = CAPTION_GLYPH_SIZE;
    match button {
        WindowButton::Minimize => div().w(glyph).h(px(1.0)).bg(color).into_any_element(),
        WindowButton::Maximize if maximized => {
            // Two overlapping outlines: the back one only shows its top and right strokes.
            let side = glyph - px(2.0);
            div()
                .relative()
                .size(glyph)
                .child(
                    div()
                        .absolute()
                        .left(px(2.0))
                        .top(px(0.0))
                        .size(side)
                        .border_t_1()
                        .border_r_1()
                        .border_color(color),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(2.0))
                        .size(side)
                        .border_1()
                        .border_color(color),
                )
                .into_any_element()
        }
        WindowButton::Maximize => div()
            .size(glyph)
            .border_1()
            .border_color(color)
            .into_any_element(),
        WindowButton::Close => canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                // The control is hovered or pressed while the pointer is inside it; the glyph
                // canvas is centered in the control.
                let control = crate::Bounds::from_corners(
                    bounds.origin
                        - point(
                            (control_size.width - glyph) / 2.0,
                            (control_size.height - glyph) / 2.0,
                        ),
                    bounds.origin
                        + point(
                            glyph + (control_size.width - glyph) / 2.0,
                            glyph + (control_size.height - glyph) / 2.0,
                        ),
                );
                let hovered =
                    control.contains(&window.mouse_position()) && !window.last_input_was_keyboard();
                let color: Background = if hovered { hover_color } else { color }.into();
                let (min, max) = (bounds.origin, bounds.origin + point(glyph, glyph));
                for (from, to) in [(min, max), (point(min.x, max.y), point(max.x, min.y))] {
                    let mut stroke = PathBuilder::stroke(px(1.0));
                    stroke.move_to(from);
                    stroke.line_to(to);
                    if let Ok(path) = stroke.build() {
                        window.paint_path(path, color);
                    }
                }
            },
        )
        .size(glyph)
        .into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Bounds, Context, MouseDownEvent, ParentElement, Render, Styled, TestApp, TestAppWindow,
        TestChromeRequest, TestWindow, WindowOptions, point, transparent_black,
    };
    use std::{cell::Cell, rc::Rc};

    fn geometry() -> WindowChromeGeometry {
        WindowChromeGeometry {
            size: size(px(800.0), px(480.0)),
            tiling: Tiling::default(),
            maximized: false,
            fullscreen: false,
            resizable: true,
        }
    }

    fn edge_at(geometry: WindowChromeGeometry, x: f32, y: f32) -> Option<ResizeEdge> {
        geometry.resize_edge_at(point(px(x), px(y)))
    }

    #[test]
    fn resize_edge_bands_and_corners_on_a_free_window() {
        let g = geometry();
        assert_eq!(edge_at(g, 2.0, 240.0), Some(ResizeEdge::Left));
        assert_eq!(edge_at(g, 797.0, 240.0), Some(ResizeEdge::Right));
        assert_eq!(edge_at(g, 400.0, 2.0), Some(ResizeEdge::Top));
        assert_eq!(edge_at(g, 400.0, 477.0), Some(ResizeEdge::Bottom));
        assert_eq!(edge_at(g, 2.0, 2.0), Some(ResizeEdge::TopLeft));
        assert_eq!(edge_at(g, 13.0, 5.0), Some(ResizeEdge::TopLeft));
        assert_eq!(edge_at(g, 14.0, 5.0), Some(ResizeEdge::Top));
        assert_eq!(edge_at(g, 790.0, 2.0), Some(ResizeEdge::TopRight));
        assert_eq!(edge_at(g, 2.0, 20.0), Some(ResizeEdge::Left));
        assert_eq!(edge_at(g, 6.0, 240.0), None);
        assert_eq!(edge_at(g, 400.0, 6.0), None);

        // The remaining corners and the L-shaped corner region: 14 px along, 6 px deep.
        assert_eq!(edge_at(g, 797.0, 477.0), Some(ResizeEdge::BottomRight));
        assert_eq!(edge_at(g, 3.0, 470.0), Some(ResizeEdge::BottomLeft));
        assert_eq!(edge_at(g, 3.0, 13.0), Some(ResizeEdge::TopLeft));
        assert_eq!(edge_at(g, 3.0, 14.0), Some(ResizeEdge::Left));
        assert_eq!(edge_at(g, 400.0, 474.0), None);
        assert_eq!(edge_at(g, 400.0, 475.0), Some(ResizeEdge::Bottom));
    }

    #[test]
    fn resize_edges_follow_tiled_edges() {
        let mut g = geometry();
        g.tiling.left = true;
        // The tiled left edge belongs to the compositor; its diagonal is not offered either, so
        // the adjacent free edge applies along its full length including the corner span.
        assert_eq!(edge_at(g, 2.0, 240.0), None);
        assert_eq!(edge_at(g, 400.0, 2.0), Some(ResizeEdge::Top));
        assert_eq!(edge_at(g, 2.0, 2.0), Some(ResizeEdge::Top));
        assert_eq!(edge_at(g, 2.0, 477.0), Some(ResizeEdge::Bottom));
        assert_eq!(edge_at(g, 2.0, 10.0), None);
        assert_eq!(edge_at(g, 797.0, 240.0), Some(ResizeEdge::Right));
        assert_eq!(edge_at(g, 790.0, 2.0), Some(ResizeEdge::TopRight));
    }

    #[test]
    fn no_resize_areas_when_maximized_fullscreen_or_not_resizable() {
        let points = [
            (2.0, 240.0),
            (797.0, 240.0),
            (400.0, 2.0),
            (400.0, 477.0),
            (2.0, 2.0),
            (13.0, 5.0),
            (790.0, 2.0),
            (797.0, 477.0),
            (2.0, 477.0),
        ];
        let variants = [
            WindowChromeGeometry {
                maximized: true,
                ..geometry()
            },
            WindowChromeGeometry {
                fullscreen: true,
                ..geometry()
            },
            WindowChromeGeometry {
                resizable: false,
                ..geometry()
            },
        ];
        for g in variants {
            for (x, y) in points {
                assert_eq!(edge_at(g, x, y), None, "{g:?} at ({x}, {y})");
            }
        }
    }

    #[test]
    fn points_outside_the_window_resolve_to_nothing() {
        let g = geometry();
        assert_eq!(edge_at(g, -1.0, 240.0), None);
        assert_eq!(edge_at(g, 801.0, 240.0), None);
        assert_eq!(edge_at(g, 400.0, 481.0), None);
    }

    fn radii(g: WindowChromeGeometry) -> [f32; 4] {
        let c = g.corner_radii();
        [
            f32::from(c.top_left),
            f32::from(c.top_right),
            f32::from(c.bottom_right),
            f32::from(c.bottom_left),
        ]
    }

    #[test]
    fn corner_shape_follows_tiling_maximized_and_fullscreen() {
        assert_eq!(radii(geometry()), [10.0; 4]);

        let mut top_tiled = geometry();
        top_tiled.tiling.top = true;
        // [top-left, top-right, bottom-right, bottom-left]
        assert_eq!(radii(top_tiled), [0.0, 0.0, 10.0, 10.0]);

        let mut left_tiled = geometry();
        left_tiled.tiling.left = true;
        assert_eq!(radii(left_tiled), [0.0, 10.0, 10.0, 0.0]);

        let maximized = WindowChromeGeometry {
            maximized: true,
            tiling: Tiling::tiled(),
            ..geometry()
        };
        assert_eq!(radii(maximized), [0.0; 4]);
        // Even when the platform reports no tiling for a maximized or fullscreen window.
        assert_eq!(
            radii(WindowChromeGeometry {
                maximized: true,
                ..geometry()
            }),
            [0.0; 4]
        );
        assert_eq!(
            radii(WindowChromeGeometry {
                fullscreen: true,
                ..geometry()
            }),
            [0.0; 4]
        );
    }

    #[test]
    fn client_inset_is_zero_on_tiled_maximized_and_fullscreen_edges() {
        let inset = |g: WindowChromeGeometry| {
            let e = g.client_inset(px(10.0));
            [
                f32::from(e.top),
                f32::from(e.right),
                f32::from(e.bottom),
                f32::from(e.left),
            ]
        };
        assert_eq!(inset(geometry()), [10.0; 4]);
        let mut top_tiled = geometry();
        top_tiled.tiling.top = true;
        assert_eq!(inset(top_tiled), [0.0, 10.0, 10.0, 10.0]);
        assert_eq!(
            inset(WindowChromeGeometry {
                maximized: true,
                ..geometry()
            }),
            [0.0; 4]
        );
        assert_eq!(
            inset(WindowChromeGeometry {
                fullscreen: true,
                ..geometry()
            }),
            [0.0; 4]
        );
    }

    #[test]
    fn resize_edge_cursors_match_their_axis() {
        assert_eq!(ResizeEdge::Top.cursor_style(), CursorStyle::ResizeUpDown);
        assert_eq!(
            ResizeEdge::Left.cursor_style(),
            CursorStyle::ResizeLeftRight
        );
        assert_eq!(
            ResizeEdge::TopLeft.cursor_style(),
            CursorStyle::ResizeUpLeftDownRight
        );
        assert_eq!(
            ResizeEdge::TopRight.cursor_style(),
            CursorStyle::ResizeUpRightDownLeft
        );
    }

    #[test]
    fn background_fallback_rules() {
        let none = WindowBackgroundCapabilities::UNSUPPORTED;
        let transparency_only = WindowBackgroundCapabilities {
            transparency: true,
            blur: false,
        };
        let all = WindowBackgroundCapabilities::ALL;
        use WindowBackgroundAppearance::{Blurred, Opaque, Transparent};

        assert_eq!(none.effective(Transparent), Opaque);
        assert_eq!(none.effective(Blurred), Opaque);
        assert_eq!(transparency_only.effective(Transparent), Transparent);
        assert_eq!(transparency_only.effective(Blurred), Opaque);
        assert_eq!(all.effective(Blurred), Blurred);
        for caps in [none, transparency_only, all] {
            assert_eq!(caps.effective(Opaque), Opaque);
        }

        assert!(none.requires_opaque_fallback(Opaque));
        assert!(!transparency_only.requires_opaque_fallback(Opaque));
        assert!(transparency_only.requires_opaque_fallback(Blurred));
        assert!(!all.requires_opaque_fallback(Blurred));
    }

    fn sides(layout: &WindowButtonLayout) -> (Vec<WindowButton>, Vec<WindowButton>) {
        (
            layout.left.iter().flatten().copied().collect(),
            layout.right.iter().flatten().copied().collect(),
        )
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn button_layout_parsing_rules() {
        use WindowButton::{Close, Maximize, Minimize};
        let parse = |s: &str| WindowButtonLayout::parse(s).map(|layout| sides(&layout));

        assert_eq!(
            parse("close,minimize:maximize").unwrap(),
            (vec![Close, Minimize], vec![Maximize])
        );
        assert_eq!(
            parse(":minimize,maximize,close").unwrap(),
            (vec![], vec![Minimize, Maximize, Close])
        );
        assert_eq!(parse("close,close:close").unwrap(), (vec![Close], vec![]));
        assert!(parse("appmenu:").is_err());
        assert!(parse("foo").is_err());
        assert_eq!(
            parse("minimize,maximize,close,minimize:").unwrap(),
            (vec![Minimize, Maximize, Close], vec![])
        );
        // A string without a colon is the right-hand list.
        assert_eq!(parse("close").unwrap(), (vec![], vec![Close]));
        // Unknown names are ignored when something valid remains; empty items are skipped.
        assert_eq!(
            parse("appmenu,close,,minimize:maximize").unwrap(),
            (vec![Close, Minimize], vec![Maximize])
        );
        // The unset/unparsable fallback is nothing on the left, minimize maximize close on the right.
        assert_eq!(
            sides(&default_caption_button_layout()),
            (vec![], vec![Minimize, Maximize, Close])
        );
    }

    #[test]
    fn at_most_three_buttons_per_side_survive_parsing() {
        // Three distinct buttons fill a side; the layout type cannot hold a fourth.
        let layout = default_caption_button_layout();
        assert_eq!(layout.left.len(), crate::MAX_BUTTONS_PER_SIDE);
        assert_eq!(layout.right.len(), crate::MAX_BUTTONS_PER_SIDE);
    }

    #[test]
    fn caption_buttons_respect_support_resizable_and_minimizable() {
        use CaptionControlsSide::Right;
        use WindowButton::{Close, Maximize, Minimize};
        let layout = default_caption_button_layout();
        let all = WindowControls::default();
        let visible = |controls, resizable, minimizable| {
            visible_caption_buttons(&layout, Right, controls, resizable, minimizable)
        };

        assert_eq!(visible(all, true, true), vec![Minimize, Maximize, Close]);
        assert_eq!(visible(all, true, false), vec![Maximize, Close]);
        assert_eq!(visible(all, false, true), vec![Minimize, Close]);
        assert_eq!(
            visible(
                WindowControls {
                    minimize: false,
                    maximize: false,
                    ..all
                },
                true,
                true
            ),
            vec![Close]
        );
        assert!(
            visible_caption_buttons(&layout, CaptionControlsSide::Left, all, true, true).is_empty()
        );
    }

    #[test]
    fn caption_controls_have_accessible_names() {
        assert_eq!(
            caption_control_name(WindowButton::Minimize, false),
            "Minimize"
        );
        assert_eq!(
            caption_control_name(WindowButton::Maximize, false),
            "Maximize"
        );
        assert_eq!(
            caption_control_name(WindowButton::Maximize, true),
            "Restore"
        );
        assert_eq!(caption_control_name(WindowButton::Close, false), "Close");
    }

    #[test]
    fn caption_controls_default_to_the_documented_hit_region() {
        let style = CaptionControlsStyle::default();
        assert_eq!(style.control_size, size(px(40.0), px(38.0)));
    }

    /// Shared observation points of the test view.
    #[derive(Clone, Default)]
    struct Probe {
        renders: Rc<Cell<usize>>,
        body_presses: Rc<Cell<usize>>,
        body_clicks: Rc<Cell<usize>>,
        body_starts_move: Rc<Cell<bool>>,
    }

    struct ChromeView {
        probe: Probe,
    }

    impl Render for ChromeView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let probe = self.probe.clone();
            probe.renders.set(probe.renders.get() + 1);
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(window_resize_handles())
                .child(
                    div()
                        .id("titlebar")
                        .h(px(38.0))
                        .w_full()
                        .flex()
                        .flex_row()
                        .window_drag_region()
                        .child(CaptionControls::new(CaptionControlsSide::Left))
                        .child(div().flex_1())
                        .child(CaptionControls::new(CaptionControlsSide::Right)),
                )
                .child(
                    div()
                        .id("body")
                        .flex_1()
                        .on_mouse_down(MouseButton::Left, {
                            let probe = probe.clone();
                            move |_, window, _| {
                                probe.body_presses.set(probe.body_presses.get() + 1);
                                if probe.body_starts_move.get() {
                                    window.start_window_move();
                                }
                            }
                        })
                        .on_click(move |_, _, _| {
                            probe.body_clicks.set(probe.body_clicks.get() + 1)
                        }),
                )
        }
    }

    struct Fixture {
        app: TestApp,
        window: TestAppWindow<ChromeView>,
        test_window: TestWindow,
        probe: Probe,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_options(WindowOptions::default())
        }

        fn with_options(options: WindowOptions) -> Self {
            let mut app = TestApp::new();
            let probe = Probe::default();
            let bounds = app.read(|cx| Bounds::maximized(None, cx));
            let options = WindowOptions {
                window_bounds: Some(crate::WindowBounds::Windowed(bounds)),
                ..options
            };
            let mut window = app.open_window_with_options(options, {
                let probe = probe.clone();
                |_, _| ChromeView { probe }
            });
            let test_window = window.update(|_, window, _| {
                window
                    .platform_window
                    .as_test()
                    .cloned()
                    .expect("test platform window")
            });
            let mut fixture = Self {
                app,
                window,
                test_window,
                probe,
            };
            fixture.redraw();
            // Window creation itself requests decorations; the tests look at what follows.
            fixture.test_window.clear_chrome_requests();
            fixture
        }

        /// Re-renders the window so it reflects the fake platform's current state.
        fn redraw(&mut self) {
            self.window.update(|_, window, _| window.refresh());
            self.window.draw();
        }

        fn client(&mut self, tiling: Tiling) {
            self.test_window
                .set_decorations(Decorations::Client { tiling });
            self.redraw();
        }

        fn requests(&self) -> Vec<TestChromeRequest> {
            self.test_window.chrome_requests()
        }

        /// Window-control hitboxes of the last frame, left to right, excluding drag regions.
        fn controls(&mut self) -> Vec<WindowControlArea> {
            let mut controls = self.window.update(|_, window, _| {
                window
                    .rendered_frame
                    .window_control_hitboxes
                    .iter()
                    .filter(|(area, _)| *area != WindowControlArea::Drag)
                    .map(|(area, hitbox)| (hitbox.bounds.origin.x, *area))
                    .collect::<Vec<_>>()
            });
            controls.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            controls.into_iter().map(|(_, area)| area).collect()
        }

        fn control_center(&mut self, wanted: WindowControlArea) -> Point<Pixels> {
            self.window.update(|_, window, _| {
                window
                    .rendered_frame
                    .window_control_hitboxes
                    .iter()
                    .find(|(area, _)| *area == wanted)
                    .map(|(_, hitbox)| hitbox.bounds.center())
                    .expect("control present")
            })
        }

        fn viewport(&mut self) -> Size<Pixels> {
            self.window.update(|_, window, _| window.viewport_size())
        }
    }

    fn mouse_down(
        position: Point<Pixels>,
        button: MouseButton,
        click_count: usize,
    ) -> MouseDownEvent {
        MouseDownEvent {
            position,
            button,
            modifiers: Default::default(),
            click_count,
            first_mouse: false,
        }
    }

    #[test]
    fn server_decorations_render_no_controls_but_the_drag_region_moves() {
        let mut f = Fixture::new();
        assert_eq!(
            f.test_window.0.lock().chrome.decorations,
            Decorations::Server
        );
        assert!(f.controls().is_empty());

        f.window
            .simulate_mouse_down(point(px(300.0), px(10.0)), MouseButton::Left);
        assert_eq!(f.requests(), vec![TestChromeRequest::Move]);
    }

    #[test]
    fn client_decorations_render_exactly_the_permitted_controls() {
        let mut f = Fixture::new();
        f.client(Tiling::default());
        assert_eq!(
            f.controls(),
            vec![
                WindowControlArea::Min,
                WindowControlArea::Max,
                WindowControlArea::Close
            ]
        );

        f.test_window.set_window_controls(WindowControls {
            fullscreen: true,
            maximize: false,
            minimize: true,
            window_menu: true,
        });
        f.redraw();
        assert_eq!(
            f.controls(),
            vec![WindowControlArea::Min, WindowControlArea::Close]
        );
    }

    #[test]
    fn minimize_is_absent_for_a_non_minimizable_window() {
        let mut f = Fixture::with_options(WindowOptions {
            is_minimizable: false,
            ..Default::default()
        });
        f.client(Tiling::default());
        assert_eq!(
            f.controls(),
            vec![WindowControlArea::Max, WindowControlArea::Close]
        );

        let mut f = Fixture::with_options(WindowOptions {
            is_resizable: false,
            ..Default::default()
        });
        f.client(Tiling::default());
        assert_eq!(
            f.controls(),
            vec![WindowControlArea::Min, WindowControlArea::Close]
        );
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn caption_controls_follow_the_desktop_button_layout() {
        let mut f = Fixture::new();
        f.client(Tiling::default());
        f.test_window.simulate_button_layout_change(
            WindowButtonLayout::parse("close,minimize:maximize").ok(),
        );
        f.redraw();
        assert_eq!(
            f.controls(),
            vec![
                WindowControlArea::Close,
                WindowControlArea::Min,
                WindowControlArea::Max
            ]
        );
    }

    #[test]
    fn caption_controls_activate_on_press_and_release_inside() {
        let mut f = Fixture::new();
        f.client(Tiling::default());
        let minimize = f.control_center(WindowControlArea::Min);
        let maximize = f.control_center(WindowControlArea::Max);

        f.window.simulate_click(minimize, MouseButton::Left);
        assert_eq!(f.requests(), vec![TestChromeRequest::Minimize]);

        f.window.simulate_click(maximize, MouseButton::Left);
        assert_eq!(
            f.requests(),
            vec![
                TestChromeRequest::Minimize,
                TestChromeRequest::ToggleMaximize
            ]
        );
        f.redraw();
        // Maximized: the same control now toggles back.
        let maximize = f.control_center(WindowControlArea::Max);
        f.window.simulate_click(maximize, MouseButton::Left);
        assert_eq!(
            f.requests().last(),
            Some(&TestChromeRequest::ToggleMaximize)
        );
        assert!(!f.window.update(|_, window, _| window.is_maximized()));

        // Press on a control, release elsewhere: cancelled. A press on a control never starts a move.
        let before = f.requests().len();
        f.window.simulate_mouse_down(minimize, MouseButton::Left);
        f.window
            .simulate_mouse_up(point(px(300.0), px(200.0)), MouseButton::Left);
        assert_eq!(f.requests().len(), before);
    }

    #[test]
    fn closing_goes_through_the_close_decision() {
        let mut f = Fixture::new();
        f.client(Tiling::default());
        let decisions = Rc::new(Cell::new(0));
        let allow = Rc::new(Cell::new(false));
        f.window.update(|_, window, cx| {
            let (decisions, allow) = (decisions.clone(), allow.clone());
            window.on_window_should_close(cx, move |_, _| {
                decisions.set(decisions.get() + 1);
                allow.get()
            });
        });

        let close = f.control_center(WindowControlArea::Close);
        f.window.simulate_click(close, MouseButton::Left);
        assert_eq!(decisions.get(), 1);
        assert_eq!(f.app.windows().len(), 1, "a vetoed close leaves the window");

        allow.set(true);
        f.window.simulate_click(close, MouseButton::Left);
        assert_eq!(decisions.get(), 2);
        assert!(f.app.windows().is_empty());
    }

    #[test]
    fn drag_region_double_click_toggles_maximize_when_resizable() {
        let mut f = Fixture::new();
        f.window
            .simulate_event(mouse_down(point(px(300.0), px(10.0)), MouseButton::Left, 2));
        assert_eq!(f.requests(), vec![TestChromeRequest::ToggleMaximize]);

        let mut f = Fixture::with_options(WindowOptions {
            is_resizable: false,
            ..Default::default()
        });
        f.window
            .simulate_event(mouse_down(point(px(300.0), px(10.0)), MouseButton::Left, 2));
        assert!(f.requests().is_empty());
    }

    #[test]
    fn drag_region_secondary_press_opens_the_window_menu_when_supported() {
        let mut f = Fixture::new();
        let position = point(px(300.0), px(10.0));
        f.window.simulate_mouse_down(position, MouseButton::Right);
        assert_eq!(f.requests(), vec![TestChromeRequest::WindowMenu(position)]);

        let mut f = Fixture::new();
        f.test_window.set_window_controls(WindowControls {
            window_menu: false,
            ..WindowControls::default()
        });
        f.window.simulate_mouse_down(position, MouseButton::Right);
        assert!(f.requests().is_empty());
    }

    #[test]
    fn drag_region_does_not_start_a_move_from_the_body() {
        let mut f = Fixture::new();
        f.window
            .simulate_mouse_down(point(px(300.0), px(200.0)), MouseButton::Left);
        assert!(f.requests().is_empty());
        assert_eq!(f.probe.body_presses.get(), 1);
    }

    #[test]
    fn native_move_and_resize_without_a_pointer_press_are_ignored() {
        let mut f = Fixture::new();
        f.window.update(|_, window, _| {
            window.start_window_move();
            window.start_window_resize(ResizeEdge::Left);
        });
        assert!(f.requests().is_empty());
    }

    #[test]
    fn native_move_releases_the_pressed_state() {
        let mut f = Fixture::new();
        f.probe.body_starts_move.set(true);
        let position = point(px(300.0), px(200.0));

        f.window.simulate_mouse_down(position, MouseButton::Left);
        assert_eq!(f.requests(), vec![TestChromeRequest::Move]);
        assert_eq!(
            f.window
                .update(|_, window, _| window.pressed_pointer_button()),
            None
        );
        // The pressed element lost its pending press: the eventual release is not a click.
        f.window.simulate_mouse_up(position, MouseButton::Left);
        assert_eq!(f.probe.body_clicks.get(), 0);

        // The move began from a press; with the press consumed, another request is a no-op.
        f.window.update(|_, window, _| window.start_window_move());
        assert_eq!(f.requests(), vec![TestChromeRequest::Move]);

        // Ordinary presses are unaffected.
        f.probe.body_starts_move.set(false);
        f.window.simulate_click(position, MouseButton::Left);
        assert_eq!(f.probe.body_clicks.get(), 1);
        assert_eq!(f.requests(), vec![TestChromeRequest::Move]);
    }

    #[test]
    fn pressed_pointer_button_is_tracked_between_press_and_release() {
        let mut f = Fixture::new();
        let position = point(px(300.0), px(200.0));
        let pressed = |f: &mut Fixture| {
            f.window
                .update(|_, window, _| window.pressed_pointer_button())
        };
        assert_eq!(pressed(&mut f), None);
        f.window.simulate_mouse_down(position, MouseButton::Left);
        assert_eq!(pressed(&mut f), Some(MouseButton::Left));
        f.window.simulate_mouse_up(position, MouseButton::Left);
        assert_eq!(pressed(&mut f), None);
    }

    #[test]
    fn edge_presses_start_native_resizes_before_children_see_them() {
        let mut f = Fixture::new();
        // Server-decorated: the compositor owns the edges.
        f.window
            .simulate_mouse_down(point(px(2.0), px(300.0)), MouseButton::Left);
        assert!(f.requests().is_empty());
        assert_eq!(f.probe.body_presses.get(), 1);
        f.window
            .simulate_mouse_up(point(px(2.0), px(300.0)), MouseButton::Left);

        f.client(Tiling::default());
        let size = f.viewport();
        f.window
            .simulate_mouse_down(point(px(2.0), px(300.0)), MouseButton::Left);
        assert_eq!(
            f.requests(),
            vec![TestChromeRequest::Resize(ResizeEdge::Left)]
        );
        assert_eq!(
            f.probe.body_presses.get(),
            1,
            "the body never saw the edge press"
        );

        f.window
            .simulate_mouse_up(point(px(2.0), px(300.0)), MouseButton::Left);
        f.window.simulate_mouse_down(
            point(size.width - px(2.0), size.height - px(2.0)),
            MouseButton::Left,
        );
        assert_eq!(
            f.requests().last(),
            Some(&TestChromeRequest::Resize(ResizeEdge::BottomRight))
        );
    }

    #[test]
    fn window_chrome_queries_follow_the_platform_state() {
        let mut f = Fixture::new();
        let state = |f: &mut Fixture| {
            f.window.update(|_, window, _| {
                (
                    window.is_client_decorated(),
                    window.window_corner_radii(),
                    window.effective_client_inset(),
                    window.resize_edge_at(point(px(2.0), px(300.0))),
                )
            })
        };

        // Server-decorated: no rounding, no inset, no client resize areas.
        let (client, radii, inset, edge) = state(&mut f);
        assert!(!client);
        assert_eq!(radii, Corners::all(px(0.0)));
        assert_eq!(inset, Edges::all(px(0.0)));
        assert_eq!(edge, None);

        f.client(Tiling::default());
        f.window
            .update(|_, window, _| window.set_client_inset(px(10.0)));
        let (client, radii, inset, edge) = state(&mut f);
        assert!(client);
        assert_eq!(radii, Corners::all(px(10.0)));
        assert_eq!(inset, Edges::all(px(10.0)));
        assert_eq!(edge, Some(ResizeEdge::Left));

        f.client(Tiling {
            top: true,
            ..Tiling::default()
        });
        let (_, radii, inset, _) = state(&mut f);
        assert_eq!(radii.top_left, px(0.0));
        assert_eq!(radii.top_right, px(0.0));
        assert_eq!(radii.bottom_left, px(10.0));
        assert_eq!(radii.bottom_right, px(10.0));
        assert_eq!(inset.top, px(0.0));
        assert_eq!(inset.bottom, px(10.0));

        f.test_window.set_maximized(true);
        let (_, radii, inset, edge) = state(&mut f);
        assert_eq!(radii, Corners::all(px(0.0)));
        assert_eq!(inset, Edges::all(px(0.0)));
        assert_eq!(edge, None);
    }

    #[test]
    fn requested_blur_without_blur_support_falls_back_to_opaque() {
        let mut f = Fixture::new();
        f.client(Tiling::default());
        f.test_window
            .set_background_capabilities(WindowBackgroundCapabilities {
                transparency: true,
                blur: false,
            });
        f.window.update(|_, window, _| {
            window.set_client_inset(px(10.0));
            window.set_background_appearance(WindowBackgroundAppearance::Blurred);
        });

        let (effective, requested, fallback, radii, inset, background, pushed) =
            f.window.update(|_, window, _| {
                (
                    window.effective_background_appearance(),
                    window.requested_background_appearance(),
                    window.is_opaque_fallback(),
                    window.window_corner_radii(),
                    window.effective_client_inset(),
                    window
                        .resolve_window_background(hsla(0.0, 0.0, 0.5, 0.4), rgb(0xffffff).into()),
                    window.client_inset(),
                )
            });
        assert_eq!(effective, WindowBackgroundAppearance::Opaque);
        assert_eq!(requested, WindowBackgroundAppearance::Blurred);
        assert!(fallback);
        assert_eq!(radii, Corners::all(px(0.0)));
        assert_eq!(inset, Edges::all(px(0.0)));
        assert_eq!(background.a, 1.0, "no transparent pixels");
        assert_eq!(pushed, Some(px(10.0)));
        // The platform was told the effective (zero) inset.
        assert_eq!(
            f.requests().last(),
            Some(&TestChromeRequest::ClientInset(px(0.0)))
        );
    }

    #[test]
    fn capability_query_reports_the_platform_answers() {
        let mut f = Fixture::new();
        for capabilities in [
            WindowBackgroundCapabilities::UNSUPPORTED,
            WindowBackgroundCapabilities {
                transparency: true,
                blur: false,
            },
            WindowBackgroundCapabilities::ALL,
        ] {
            f.test_window.set_background_capabilities(capabilities);
            assert_eq!(
                f.window
                    .update(|_, window, _| window.background_capabilities()),
                capabilities
            );
        }
    }

    #[test]
    fn blur_is_removed_when_the_appearance_returns_to_opaque() {
        let mut f = Fixture::new();
        f.client(Tiling::default());
        f.test_window
            .set_background_capabilities(WindowBackgroundCapabilities::ALL);
        let effective = |f: &mut Fixture| {
            f.window
                .update(|_, window, _| window.effective_background_appearance())
        };

        f.window.update(|_, window, _| {
            window.set_background_appearance(WindowBackgroundAppearance::Blurred)
        });
        assert_eq!(effective(&mut f), WindowBackgroundAppearance::Blurred);
        assert_eq!(
            f.test_window.requested_background(),
            WindowBackgroundAppearance::Blurred
        );

        f.window.update(|_, window, _| {
            window.set_background_appearance(WindowBackgroundAppearance::Opaque)
        });
        assert_eq!(effective(&mut f), WindowBackgroundAppearance::Opaque);
        assert_eq!(
            f.test_window.requested_background(),
            WindowBackgroundAppearance::Opaque
        );
    }

    #[test]
    fn opaque_fallback_paints_an_opaque_window_background() {
        struct Backdrop;
        impl Render for Backdrop {
            fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .bg(window.resolve_window_background(transparent_black(), rgb(0xfafafa).into()))
            }
        }

        let mut app = TestApp::new();
        let mut window = app.open_window(|_, _| Backdrop);
        let test_window =
            window.update(|_, window, _| window.platform_window.as_test().cloned().unwrap());
        // A transparent request that the platform cannot honour resolves to the opaque color.
        test_window.set_background_capabilities(WindowBackgroundCapabilities::UNSUPPORTED);
        window.update(|_, window, _| {
            window.set_background_appearance(WindowBackgroundAppearance::Transparent);
            window.refresh();
        });
        window.draw();
        let alphas = window.update(|_, window, _| {
            window
                .painted_quads()
                .iter()
                .map(|quad| quad.background.solid.a)
                .collect::<Vec<_>>()
        });
        assert!(!alphas.is_empty());
        assert!(alphas.iter().all(|alpha| *alpha == 1.0), "{alphas:?}");
    }

    #[test]
    fn chrome_changes_cause_a_new_frame() {
        let mut f = Fixture::new();
        let frame = |f: &Fixture| {
            f.test_window
                .simulate_frame_request(crate::RequestFrameOptions::default())
        };
        frame(&f);
        let rendered = f.probe.renders.get();

        // Nothing changed: no new frame is drawn.
        frame(&f);
        assert_eq!(f.probe.renders.get(), rendered);

        let steps: [(&str, Box<dyn Fn(&TestWindow)>); 4] = [
            (
                "decoration mode",
                Box::new(|w| {
                    w.set_decorations(Decorations::Client {
                        tiling: Tiling::default(),
                    })
                }),
            ),
            (
                "tiling",
                Box::new(|w| {
                    w.set_decorations(Decorations::Client {
                        tiling: Tiling {
                            left: true,
                            ..Tiling::default()
                        },
                    })
                }),
            ),
            ("maximized", Box::new(|w| w.set_maximized(true))),
            (
                "supported controls",
                Box::new(|w| {
                    w.set_window_controls(WindowControls {
                        window_menu: false,
                        ..WindowControls::default()
                    })
                }),
            ),
        ];
        for (what, change) in steps {
            let before = f.probe.renders.get();
            change(&f.test_window);
            frame(&f);
            // (Maximizing also resizes the fake window, which draws on its own.)
            assert!(f.probe.renders.get() > before, "{what} must redraw");
        }
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn button_layout_change_triggers_a_redraw() {
        let mut f = Fixture::new();
        f.client(Tiling::default());
        let observed = Rc::new(Cell::new(0));
        let observer = f.window.update(|_, window, _| {
            let observed = observed.clone();
            window.observe_button_layout_changed(move |_, _| observed.set(observed.get() + 1))
        });
        let observed_before = observed.get();
        let renders = f.probe.renders.get();
        f.test_window
            .simulate_button_layout_change(WindowButtonLayout::parse("close:").ok());
        assert_eq!(observed.get(), observed_before + 1);
        assert!(
            f.probe.renders.get() > renders,
            "the layout change must redraw"
        );
        drop(observer);
    }
}
