//! The horizontal scroll indicator of code blocks (CMP-014 R9): an overlay thumb over the bottom
//! edge with a 10 px hit area, drawn in the faint-text token at alpha 0.5 (0.68 when hovered or
//! dragged), at the resting (3 px) or active (5 px) thickness of the shared overlay scrollbar
//! (CMP-031). The scroll offset lives in [`CodeScrollState`], owned by the caller and keyed by
//! block so a re-render keeps the pan.

use std::cell::Cell;
use std::rc::Rc;
use vespertine::{
    Bounds, CursorStyle, DispatchPhase, HitboxBehavior, Hsla, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, Styled, canvas, fill,
    point, px, size,
};

use crate::scroll::{
    RAIL_HIT_THICKNESS, RAIL_THUMB_ACTIVE_THICKNESS, RAIL_THUMB_EDGE_OFFSET, RAIL_THUMB_THICKNESS,
    RAIL_TRACK_INSET, RailMetrics,
};

/// Thumb alpha at rest.
const ALPHA_REST: f32 = 0.5;
/// Thumb alpha while hovered or dragged.
const ALPHA_ACTIVE: f32 = 0.68;

/// Owner-controlled scroll state of one code block: the scroll handle of its horizontal viewport
/// plus the indicator's hover and drag state.
#[derive(Clone, Debug, Default)]
pub struct CodeScrollState {
    handle: ScrollHandle,
    hovered: Rc<Cell<bool>>,
    grab: Rc<Cell<Option<f32>>>,
}

impl CodeScrollState {
    /// A fresh state at scroll offset zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// The scroll handle of the block's horizontal viewport.
    pub fn handle(&self) -> &ScrollHandle {
        &self.handle
    }

    /// The horizontal scroll offset (zero or positive: how far the content is panned left).
    pub fn offset_x(&self) -> Pixels {
        -self.handle.offset().x
    }

    /// Pans the content to `x` (clamped at paint time to the scrollable range).
    pub fn set_offset_x(&self, x: Pixels) {
        let mut offset = self.handle.offset();
        offset.x = -x;
        self.handle.set_offset(offset);
    }

    /// Whether the thumb is being dragged.
    pub fn dragging(&self) -> bool {
        self.grab.get().is_some()
    }

    /// Whether the rail is hovered.
    pub fn hovered(&self) -> bool {
        self.hovered.get()
    }
}

/// Builds the indicator overlay element: absolutely positioned over the bottom edge of its
/// (relative) parent, spanning the block width.
pub fn scroll_indicator(state: &CodeScrollState, thumb: Hsla) -> impl IntoElement {
    let state = state.clone();
    canvas(
        {
            let state = state.clone();
            move |bounds: Bounds<Pixels>, window, _cx| {
                let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
                let viewport = state.handle.bounds().size.width.as_f32();
                let max_scroll = state.handle.max_offset().x.as_f32();
                // Content that overflows by half a pixel or less shows no rail.
                let metrics = if max_scroll > 0.5 {
                    RailMetrics::new(viewport, max_scroll, -state.handle.offset().x.as_f32())
                } else {
                    None
                };
                (hitbox, metrics)
            }
        },
        move |bounds, (hitbox, metrics), window, _cx| {
            let Some(metrics) = metrics else {
                state.hovered.set(false);
                state.grab.set(None);
                return;
            };
            let active = state.hovered.get() || state.grab.get().is_some();
            let thickness = if active {
                RAIL_THUMB_ACTIVE_THICKNESS
            } else {
                RAIL_THUMB_THICKNESS
            };
            let left = bounds.origin.x.as_f32() + RAIL_TRACK_INSET;
            let thumb_left = left + metrics.thumb_start;
            let thumb_top = bounds.bottom().as_f32() - RAIL_THUMB_EDGE_OFFSET - thickness;
            let mut color = thumb;
            color.a = if active { ALPHA_ACTIVE } else { ALPHA_REST } * thumb.a.max(0.);
            let thumb_bounds = Bounds::new(
                point(px(thumb_left), px(thumb_top)),
                size(px(metrics.thumb_len), px(thickness)),
            );
            window.paint_quad(fill(thumb_bounds, color).corner_radii(px(thickness / 2.)));
            if hitbox.is_hovered(window) {
                window.set_cursor_style(CursorStyle::Arrow, &hitbox);
            }
            let origin_x = bounds.origin.x.as_f32();
            let set_scroll = {
                let state = state.clone();
                move |scroll: f32| {
                    let mut offset = state.handle.offset();
                    offset.x = px(-scroll);
                    state.handle.set_offset(offset);
                }
            };
            window.on_mouse_event({
                let state = state.clone();
                let hitbox = hitbox.clone();
                let set_scroll = set_scroll.clone();
                move |event: &MouseDownEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || !hitbox.is_hovered(window)
                    {
                        return;
                    }
                    let x = event.position.x.as_f32() - origin_x - RAIL_TRACK_INSET;
                    let grab = metrics.grab_offset(x);
                    state.grab.set(Some(grab));
                    set_scroll(metrics.target_fraction(x, grab) * metrics.max_scroll);
                    window.refresh();
                }
            });
            window.on_mouse_event({
                let state = state.clone();
                let hitbox = hitbox.clone();
                move |event: &MouseMoveEvent, phase, window, _cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let hovered = hitbox.is_hovered(window);
                    let mut changed = hovered != state.hovered.get();
                    state.hovered.set(hovered);
                    if let Some(grab) = state.grab.get() {
                        if event.pressed_button == Some(MouseButton::Left) {
                            let x = event.position.x.as_f32() - origin_x - RAIL_TRACK_INSET;
                            set_scroll(metrics.target_fraction(x, grab) * metrics.max_scroll);
                            changed = true;
                        } else {
                            state.grab.set(None);
                            changed = true;
                        }
                    }
                    if changed {
                        window.refresh();
                    }
                }
            });
            window.on_mouse_event({
                move |event: &MouseUpEvent, phase, window, _cx| {
                    if phase == DispatchPhase::Bubble
                        && event.button == MouseButton::Left
                        && state.grab.take().is_some()
                    {
                        window.refresh();
                    }
                }
            });
        },
    )
    .absolute()
    .left_0()
    .bottom_0()
    .w_full()
    .h(px(RAIL_HIT_THICKNESS))
}
