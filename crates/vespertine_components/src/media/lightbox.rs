//! The shared full-view surface (CMP-020 R9).
//!
//! An overlay layer of priority 3 covering the whole window: a dimming scrim, the zoomable
//! viewer (90% x 85% of the window) and the image name below it. It occludes the content
//! beneath, tracks focus, closes on Escape and on a plain click (never on a drag, zoom or
//! pinch) and consumes wheel events so the page beneath never scrolls.
//!
//! The layer is a CMP-002 floating layer: [`open_lightbox`] registers it on the window's
//! dismissal stack (`floating::open_layer`), so Escape reaches it only while it is the top layer
//! and a click closes it only while nothing was opened above it. Opening captures the focus owner
//! and moves focus into the overlay; closing restores it (CMP-002 R9-R11).

use super::viewer::ImageView;
use crate::floating::{
    DismissHandler, EscapeHandler, EscapeOutcome, FocusTarget, LayerHandle, LayerId, LayerOptions,
    WeakLayerHandle, is_top_layer, open_layer, set_layer_bounds,
};
use crate::tokens::ComponentTokens;
use std::cell::OnceCell;
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
    layer: LightboxLayer,
}

/// An open full-view layer on the window's dismissal stack (CMP-002 R6-R11): Escape reaches it
/// only while it is the top layer, a plain click closes it, and closing returns focus to the
/// element that held it when the layer opened (or to the trigger when that is gone).
#[derive(Clone)]
pub struct LightboxLayer {
    layer: LayerHandle,
    focus: FocusHandle,
    on_close: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// Opens the full-view layer: joins the dismissal stack above everything already open and takes
/// focus into `focus` once the overlay is attached. `trigger` is the owning control, the focus
/// fallback when the captured focus owner is gone. `on_close` runs once for Escape, for a plain
/// click and for [`dismiss_layer`](crate::floating::dismiss_layer); render [`lightbox`] with the
/// returned handle for as long as the view is open.
pub fn open_lightbox(
    window: &mut Window,
    cx: &mut App,
    focus: &FocusHandle,
    trigger: Option<FocusHandle>,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> LightboxLayer {
    let on_close: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(on_close);
    // The Escape and dismissal handlers live in the registry entry, so they hold the layer
    // weakly: a strong handle there would keep the layer registered after its owner dropped it.
    let slot: Rc<OnceCell<WeakLayerHandle>> = Rc::new(OnceCell::new());
    let dismiss: DismissHandler = {
        let slot = slot.clone();
        let on_close = on_close.clone();
        Rc::new(move |window, cx| {
            if let Some(layer) = slot.get().and_then(WeakLayerHandle::upgrade)
                && layer.close(window, cx)
            {
                on_close(window, cx);
            }
        })
    };
    let escape: EscapeHandler = {
        let dismiss = dismiss.clone();
        Rc::new(move |window, cx| {
            dismiss(window, cx);
            EscapeOutcome::Dismissed
        })
    };
    let layer = open_layer(
        window,
        cx,
        LayerOptions {
            escape,
            dismiss,
            layer_focus: focus.clone(),
            trigger,
            initial_focus: FocusTarget::Default,
            final_focus: FocusTarget::Default,
            trigger_first: false,
        },
    );
    let _ = slot.set(layer.downgrade());
    LightboxLayer {
        layer,
        focus: focus.clone(),
        on_close,
    }
}

impl LightboxLayer {
    /// Closes the layer without running the close callback (the host decided to close it):
    /// Escape stops reaching it at once and focus is restored after the current event. Returns
    /// false when it was already closed.
    pub fn close(&self, window: &mut Window, cx: &mut App) -> bool {
        self.layer.close(window, cx)
    }

    /// The layer's identity on the window's dismissal stack.
    pub fn id(&self) -> LayerId {
        self.layer.id()
    }

    /// A plain click on the overlay: closes when this is still the top layer (a layer opened
    /// above it owns the press) and the press did not become a drag.
    fn click(&self, window: &mut Window, cx: &mut App) {
        if is_top_layer(window, cx, self.layer.id()) && self.layer.close(window, cx) {
            (self.on_close)(window, cx);
        }
    }
}

/// The full-view overlay for `target`, shown while `layer` (from [`open_lightbox`]) is open.
pub fn lightbox(target: PreviewTarget, layer: &LightboxLayer) -> Lightbox {
    Lightbox {
        target,
        layer: layer.clone(),
    }
}

impl RenderOnce for Lightbox {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Lightbox { target, layer } = self;
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

        let press_view = target.view.clone();
        let click_view = target.view.clone();
        let click_layer = layer.clone();
        let layer_id = layer.id();
        let focus = layer.focus;

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
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            if !click_view.dragged() {
                                click_layer.click(window, cx);
                            }
                        })
                        .child(
                            canvas(
                                move |bounds, window, cx| {
                                    set_layer_bounds(window, cx, layer_id, bounds);
                                },
                                |_, (), _, _| {},
                            )
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full(),
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
