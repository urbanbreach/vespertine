//! The shared full-view surface (CMP-020 R9).
//!
//! An overlay layer of priority 3 covering the whole window: a dimming scrim, the zoomable
//! viewer (90% x 85% of the window) and the image name below it. It occludes the content
//! beneath, tracks focus, closes on Escape and on a plain click (never on a drag, zoom or
//! pinch) and consumes wheel events so the page beneath never scrolls.
//!
//! The layer is built on the deferred/anchored layers of the framework (priority 3, occluding,
//! focus-tracking); the window-scoped dismissal stack of CMP-002 is not on this tree yet, so the
//! lightbox registers its own capture-phase Escape handler.

use super::viewer::ImageView;
use crate::tokens::ComponentTokens;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    Anchor, AnyElement, App, FocusHandle, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, RenderImage, RenderOnce, SharedString, Size, StatefulInteractiveElement,
    Styled, Window, anchored, canvas, deferred, div, point, px, size,
};

/// Overlay priority of the full view.
pub const LIGHTBOX_PRIORITY: usize = 3;
/// Opacity of the scrim.
pub const SCRIM_OPACITY: f32 = 0.7;
/// Width of the viewer area as a fraction of the window.
pub const VIEWER_WIDTH_FRACTION: f32 = 0.9;
/// Height of the viewer area as a fraction of the window.
pub const VIEWER_HEIGHT_FRACTION: f32 = 0.85;
/// Gap between the viewer area and the caption.
pub const CAPTION_GAP: f32 = 12.0;
/// Text size of the caption.
pub const CAPTION_SIZE: f32 = 11.0;

/// What the full view shows: a name, the image (once decoded), its natural size and the viewer
/// state that keeps zoom and pan.
#[derive(Clone)]
pub struct PreviewTarget {
    /// The name shown under the image.
    pub name: SharedString,
    /// The image, or `None` while it is still loading.
    pub image: Option<Arc<RenderImage>>,
    /// The natural size from the host (for vector images the declared logical dimensions of the
    /// prepared variant); the first frame's pixel size is used when absent.
    pub natural_size: Option<Size<Pixels>>,
    /// The viewer state (zoom, pan, drag).
    pub view: ImageView,
    /// Fill of the optional backing plate under the image.
    pub plate: Option<Hsla>,
}

impl PreviewTarget {
    /// The natural size: the host's, or the decoded image's first-frame size.
    pub fn resolved_natural_size(&self) -> Option<Size<Pixels>> {
        if let Some(natural) = self.natural_size {
            return Some(natural);
        }
        let image = self.image.as_ref()?;
        let pixels = image.size(0);
        Some(size(
            px(i32::from(pixels.width) as f32),
            px(i32::from(pixels.height) as f32),
        ))
    }
}

/// The full-view overlay element, see [`lightbox`].
#[derive(IntoElement)]
pub struct Lightbox {
    target: PreviewTarget,
    focus: FocusHandle,
    on_close: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// The full-view overlay for `target` in `window`. `focus` is the layer's focus handle (taken
/// when the layer is shown); `on_close` runs for Escape and for a plain click.
pub fn lightbox(
    _window: &Window,
    target: PreviewTarget,
    focus: &FocusHandle,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> Lightbox {
    Lightbox {
        target,
        focus: focus.clone(),
        on_close: Rc::new(on_close),
    }
}

impl RenderOnce for Lightbox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Lightbox {
            target,
            focus,
            on_close,
        } = self;
        let viewport = window.viewport_size();
        let tokens = ComponentTokens::current(cx);
        let (vw, vh) = (f32::from(viewport.width), f32::from(viewport.height));

        let body: AnyElement = match (target.resolved_natural_size(), target.image.clone()) {
            (Some(natural), Some(image)) => div()
                .w(px(vw * VIEWER_WIDTH_FRACTION))
                .h(px(vh * VIEWER_HEIGHT_FRACTION))
                .flex()
                .child(target.view.render(image, natural, None, target.plate))
                .into_any_element(),
            _ => div()
                .debug_selector(|| "lightbox-loading".into())
                .text_color(tokens.text)
                .child("Loading image")
                .into_any_element(),
        };

        let click_view = target.view.clone();
        let press_view = target.view.clone();
        let click_close = on_close.clone();
        let key_close = on_close;
        let focus_for_layer = focus.clone();

        deferred(
            anchored()
                .position(point(px(0.), px(0.)))
                .anchor(Anchor::TopLeft)
                .child(
                    div()
                        .id("media-lightbox")
                        .w(viewport.width)
                        .h(viewport.height)
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(px(CAPTION_GAP))
                        .bg(Hsla {
                            a: SCRIM_OPACITY,
                            ..tokens.scrim
                        })
                        .occlude()
                        .key_context("MediaLightbox")
                        .track_focus(&focus)
                        .capture_any_mouse_down(move |event, _, _| {
                            if event.button == MouseButton::Left {
                                press_view.begin_click();
                            }
                        })
                        .on_key_down(move |event, window, cx| {
                            if event.keystroke.key == "escape"
                                && !event.keystroke.modifiers.modified()
                            {
                                key_close(window, cx);
                                cx.stop_propagation();
                            }
                        })
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            if !click_view.dragged() {
                                click_close(window, cx);
                            }
                        })
                        .child(
                            canvas(
                                |_, _, _| {},
                                move |_, (), window, cx| {
                                    if !focus_for_layer.is_focused(window) {
                                        window.defer(cx, move |window, cx| {
                                            window.focus(&focus_for_layer, cx);
                                        });
                                    }
                                },
                            )
                            .absolute()
                            .size_0(),
                        )
                        .child(body)
                        .child(
                            div()
                                .debug_selector(|| "lightbox-caption".into())
                                .max_w(px(vw * VIEWER_WIDTH_FRACTION))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(CAPTION_SIZE))
                                .text_color(tokens.text_muted)
                                .child(target.name),
                        ),
                ),
        )
        .with_priority(LIGHTBOX_PRIORITY)
    }
}
