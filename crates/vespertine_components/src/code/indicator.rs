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

/// Height of the hit area at the bottom edge.
pub const HIT_HEIGHT: f32 = 10.;
/// Distance of the thumb from the bottom edge.
pub const THUMB_OFFSET: f32 = 2.;
/// Resting thumb thickness.
pub const THUMB_REST: f32 = 3.;
/// Thumb thickness while the rail is active.
pub const THUMB_ACTIVE: f32 = 5.;
/// Inset of the track from both ends.
pub const TRACK_INSET: f32 = 4.;
/// Smallest thumb length.
pub const MIN_THUMB: f32 = 24.;
/// Thumb alpha at rest.
pub const ALPHA_REST: f32 = 0.5;
/// Thumb alpha while hovered or dragged.
pub const ALPHA_ACTIVE: f32 = 0.68;

/// Horizontal geometry of the thumb for a viewport, content extent and scroll position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbMetrics {
    /// Length of the track (viewport minus the insets).
    pub track: f32,
    /// Offset of the thumb from the track start.
    pub start: f32,
    /// Thumb length.
    pub len: f32,
    /// Largest scroll offset (content minus viewport).
    pub max_scroll: f32,
}

impl ThumbMetrics {
    /// The metrics, or `None` when no rail is shown (a tiny track, or content that fits).
    pub fn new(viewport: f32, content: f32, scroll: f32) -> Option<Self> {
        let track = viewport - 2. * TRACK_INSET;
        let max_scroll = content - viewport;
        if viewport <= 8. || track <= 0. || max_scroll <= 0.5 || !content.is_finite() {
            return None;
        }
        let len = (track * viewport / content).max(MIN_THUMB).min(track);
        let travel = track - len;
        let start = travel * (scroll / max_scroll).clamp(0., 1.);
        Some(Self {
            track,
            start,
            len,
            max_scroll,
        })
    }

    /// Distance the thumb can travel.
    pub fn travel(&self) -> f32 {
        self.track - self.len
    }

    /// The scroll offset for a thumb start position (clamped to the track).
    pub fn scroll_for_start(&self, start: f32) -> f32 {
        let travel = self.travel();
        if travel <= 0. {
            return 0.;
        }
        (start / travel).clamp(0., 1.) * self.max_scroll
    }
}

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
                let content = viewport + state.handle.max_offset().x.as_f32();
                let metrics =
                    ThumbMetrics::new(viewport, content, -state.handle.offset().x.as_f32());
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
            let thickness = if active { THUMB_ACTIVE } else { THUMB_REST };
            let left = bounds.origin.x.as_f32() + TRACK_INSET;
            let thumb_left = left + metrics.start;
            let thumb_top = bounds.bottom().as_f32() - THUMB_OFFSET - thickness;
            let mut color = thumb;
            color.a = if active { ALPHA_ACTIVE } else { ALPHA_REST } * thumb.a.max(0.);
            let thumb_bounds = Bounds::new(
                point(px(thumb_left), px(thumb_top)),
                size(px(metrics.len), px(thickness)),
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
                    let x = event.position.x.as_f32() - origin_x - TRACK_INSET;
                    let grab = if x >= metrics.start && x <= metrics.start + metrics.len {
                        x - metrics.start
                    } else {
                        metrics.len / 2.
                    };
                    state.grab.set(Some(grab));
                    set_scroll(metrics.scroll_for_start(x - grab));
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
                            let x = event.position.x.as_f32() - origin_x - TRACK_INSET;
                            set_scroll(metrics.scroll_for_start(x - grab));
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
    .h(px(HIT_HEIGHT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_follow_the_overlay_scrollbar_rules() {
        let m = ThumbMetrics::new(320., 960., 0.).unwrap();
        assert_eq!(m.track, 312.);
        assert_eq!(m.len, 104.);
        assert_eq!(m.start, 0.);
        let m = ThumbMetrics::new(320., 960., 640.).unwrap();
        assert_eq!(m.start, 208.);
        assert_eq!(m.start, m.travel());
        // Overscroll clamps.
        assert_eq!(ThumbMetrics::new(320., 960., 99999.).unwrap().start, 208.);
        assert_eq!(ThumbMetrics::new(320., 960., -3.).unwrap().start, 0.);
        // Huge content: minimum thumb.
        assert_eq!(ThumbMetrics::new(100., 10000., 0.).unwrap().len, MIN_THUMB);
        // Nothing to scroll and tiny tracks give no rail.
        assert!(ThumbMetrics::new(320., 320., 0.).is_none());
        assert!(ThumbMetrics::new(8., 100., 0.).is_none());
    }

    #[test]
    fn dragging_maps_back_to_scroll() {
        let m = ThumbMetrics::new(320., 960., 0.).unwrap();
        assert_eq!(m.scroll_for_start(-10.), 0.);
        assert_eq!(m.scroll_for_start(208.), 640.);
        assert_eq!(m.scroll_for_start(9999.), 640.);
        assert_eq!(m.scroll_for_start(104.), 320.);
    }
}
