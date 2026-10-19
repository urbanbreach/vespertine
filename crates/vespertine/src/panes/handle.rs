//! Resize handles ("seams") between panes (LAY-004 R2).

use std::{cell::Cell, rc::Rc};

use super::fit::{PaneLimits, PaneSide};
use crate::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, GlobalElementId, Hitbox,
    HitboxBehavior, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Size, Style, Window, point, px, relative, size,
};

/// Width of a horizontal seam's hitbox, centered on the seam.
pub const HORIZONTAL_SEAM_WIDTH: Pixels = px(20.);
/// Height of the bottom seam's hitbox, centered on the seam.
pub const BOTTOM_SEAM_HEIGHT: Pixels = px(10.);

/// The hitbox rectangle of a seam.
///
/// For leading and trailing panes `seam` is the x coordinate of the seam: the
/// hitbox is [`HORIZONTAL_SEAM_WIDTH`] wide, centered on it, starting
/// `top_inset` below the top of `group` (the same inset for left and right
/// seams) and ending at the top of the bottom reservation. For the bottom pane
/// `seam` is the y coordinate of the bottom surface's top edge and the hitbox
/// is [`BOTTOM_SEAM_HEIGHT`] tall, centered on it, across the group's width.
pub fn seam_bounds(
    side: PaneSide,
    group: Bounds<Pixels>,
    seam: Pixels,
    bottom_reservation: Pixels,
    top_inset: Pixels,
) -> Bounds<Pixels> {
    if side == PaneSide::Bottom {
        Bounds::new(
            point(group.origin.x, seam - BOTTOM_SEAM_HEIGHT / 2.),
            size(group.size.width, BOTTOM_SEAM_HEIGHT),
        )
    } else {
        let inset = top_inset.max(Pixels::ZERO);
        Bounds::new(
            point(seam - HORIZONTAL_SEAM_WIDTH / 2., group.origin.y + inset),
            size(
                HORIZONTAL_SEAM_WIDTH,
                (group.size.height - bottom_reservation - inset).max(Pixels::ZERO),
            ),
        )
    }
}

type SizeCallback = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;

/// A draggable seam. The handle's own bounds are its hitbox: it consumes
/// pointer events inside them and nowhere else.
///
/// While the left button is held the pane size follows the pointer directly:
/// on every pointer move the size reported to `on_drag` is the distance from
/// the pane's outer edge to the pointer, clamped to the limits. A double-click
/// reports the default size clamped to the limits to `on_reset`.
pub struct ResizeHandle {
    id: ElementId,
    side: PaneSide,
    size: Option<Size<Pixels>>,
    outer_edge: Option<Pixels>,
    limits: PaneLimits,
    default_size: Pixels,
    on_drag: Option<SizeCallback>,
    on_reset: Option<SizeCallback>,
}

/// Creates a resize handle for a pane on `side`.
pub fn resize_handle(id: impl Into<ElementId>, side: PaneSide) -> ResizeHandle {
    ResizeHandle::new(id, side)
}

impl ResizeHandle {
    /// Creates a handle. By default it is 20 pixels wide (10 tall for the
    /// bottom pane) and fills the other axis of its parent; limits are
    /// unbounded and the reference edge is the window edge.
    pub fn new(id: impl Into<ElementId>, side: PaneSide) -> Self {
        Self {
            id: id.into(),
            side,
            size: None,
            outer_edge: None,
            limits: PaneLimits {
                min: Pixels::ZERO,
                max: Pixels::MAX,
            },
            default_size: Pixels::ZERO,
            on_drag: None,
            on_reset: None,
        }
    }

    /// Gives the handle an explicit size.
    pub fn with_size(mut self, size: Size<Pixels>) -> Self {
        self.size = Some(size);
        self
    }

    /// Sets the window coordinate of the pane's outer (or reference) edge:
    /// the left edge for a leading pane, the right edge for a trailing pane
    /// and the bottom edge for the bottom pane.
    pub fn outer_edge(mut self, edge: Pixels) -> Self {
        self.outer_edge = Some(edge);
        self
    }

    /// Sets the pane's current minimum and maximum size.
    pub fn limits(mut self, limits: PaneLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Sets the size a double-click resets to.
    pub fn default_size(mut self, size: Pixels) -> Self {
        self.default_size = size;
        self
    }

    /// Called on every pointer move of a drag with the clamped pointer-derived size.
    pub fn on_drag(mut self, f: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_drag = Some(Rc::new(f));
        self
    }

    /// Called on double-click with the default size clamped to the limits.
    pub fn on_reset(mut self, f: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_reset = Some(Rc::new(f));
        self
    }
}

impl IntoElement for ResizeHandle {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Prepaint output of a [`ResizeHandle`].
pub struct ResizeHandlePrepaint {
    hitbox: Hitbox,
    dragging: Rc<Cell<bool>>,
}

impl Element for ResizeHandle {
    type RequestLayoutState = ();
    type PrepaintState = ResizeHandlePrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        match self.size {
            Some(size) => {
                style.size.width = size.width.into();
                style.size.height = size.height.into();
            }
            None if self.side == PaneSide::Bottom => {
                style.size.width = relative(1.).into();
                style.size.height = BOTTOM_SEAM_HEIGHT.into();
            }
            None => {
                style.size.width = HORIZONTAL_SEAM_WIDTH.into();
                style.size.height = relative(1.).into();
            }
        }
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let dragging =
            window.with_element_state(global_id.unwrap(), |state: Option<Rc<Cell<bool>>>, _| {
                let state = state.unwrap_or_default();
                (state.clone(), state)
            });
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::BlockMouse);
        // An active drag follows this seam's identity (INP-001), not a frame-local hitbox id.
        window.rebind_captured_element(global_id.unwrap(), hitbox.id);
        ResizeHandlePrepaint { hitbox, dragging }
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let hitbox = prepaint.hitbox.clone();
        let dragging = prepaint.dragging.clone();
        let element_id = global_id.expect("resize handles have ids").clone();
        let side = self.side;
        let limits = self.limits;
        let default = limits.clamp(self.default_size);
        let viewport = window.viewport_size();
        let edge = self.outer_edge.unwrap_or(match side {
            PaneSide::Leading => Pixels::ZERO,
            PaneSide::Trailing => viewport.width,
            PaneSide::Bottom => viewport.height,
        });
        let on_drag = self.on_drag.clone();
        let on_reset = self.on_reset.clone();

        window.set_cursor_style(
            match side {
                PaneSide::Bottom => CursorStyle::ResizeUpDown,
                _ => CursorStyle::ResizeLeftRight,
            },
            &hitbox,
        );

        window.on_mouse_event({
            let hitbox = hitbox.clone();
            let dragging = dragging.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble
                    || event.button != MouseButton::Left
                    || !hitbox.is_hovered(window)
                {
                    return;
                }
                cx.stop_propagation();
                if event.click_count >= 2 {
                    if let Some(on_reset) = &on_reset {
                        on_reset(default, window, cx);
                    }
                } else {
                    dragging.set(true);
                    window.capture_pointer_for_element(&element_id, hitbox.id);
                }
                window.refresh();
            }
        });

        if !dragging.get() {
            return;
        }
        window.on_mouse_event({
            let dragging = dragging.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Capture || !dragging.get() {
                    return;
                }
                if event.pressed_button != Some(MouseButton::Left) {
                    // The release was lost (for example outside the window).
                    dragging.set(false);
                    window.release_pointer();
                    window.refresh();
                    return;
                }
                cx.stop_propagation();
                let derived = match side {
                    PaneSide::Leading => event.position.x - edge,
                    PaneSide::Trailing => edge - event.position.x,
                    PaneSide::Bottom => edge - event.position.y,
                };
                if let Some(on_drag) = &on_drag {
                    on_drag(limits.clamp(derived), window, cx);
                }
                window.refresh();
            }
        });

        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture
                || event.button != MouseButton::Left
                || !dragging.get()
            {
                return;
            }
            dragging.set(false);
            cx.stop_propagation();
            window.refresh();
        });
    }
}
