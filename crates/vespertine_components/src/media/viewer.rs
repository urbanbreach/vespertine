//! The zoom and pan image viewer (CMP-020 R1 to R7).
//!
//! [`ImageView`] is a cloneable handle onto the viewer state: the natural size of the image, the
//! viewport, the scale, the pan and the click-versus-drag record. Its geometry methods are pure
//! (no window needed) so they can be exercised directly; [`ImageView::render`] returns the
//! viewport element that measures itself, paints the image at exactly the displayed rectangle
//! and turns wheel, pinch and drag input into geometry changes.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use vespertine::{
    App, Bounds, ClickEvent, DispatchPhase, ElementId, EntityId, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    PinchEvent, Pixels, Point, RenderImage, ScrollWheelEvent, Size, StatefulInteractiveElement,
    Styled, TouchPhase, Window, canvas, div, point, px, size,
};

/// Distance in logical pixels a pressed pointer must travel before the press becomes a drag.
pub const DRAG_THRESHOLD: f32 = 4.0;
/// Logical pixels one wheel line is worth.
pub const LINE_PIXELS: f32 = 40.0;
/// Exponent per wheel pixel of a control-wheel zoom.
pub const WHEEL_ZOOM_RATE: f32 = 0.0025;
/// Smallest pinch factor.
pub const PINCH_FACTOR_FLOOR: f32 = 0.001;
/// Corner radius of the backing plate.
pub const PLATE_RADIUS: f32 = 10.0;
/// Bound on layout coordinates for vector images with enormous logical dimensions.
pub const LAYOUT_BOUND: f32 = 131_072.0;
/// Largest zoom scale regardless of the image size.
pub const MAX_ZOOM: f32 = 32.0;
/// Smallest zoom scale of an image that fits.
pub const MIN_ZOOM: f32 = 0.01;

/// Callback delivered a click that lands inside the displayed image.
pub type ImageClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

fn valid_size(size: Size<Pixels>) -> bool {
    let (w, h) = (f32::from(size.width), f32::from(size.height));
    w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0
}

/// The pure geometry of the viewer: natural size N, viewport V, scale s, pan p, fitted flag.
#[derive(Clone, Debug, PartialEq)]
pub struct Geometry {
    natural: (f32, f32),
    viewport: (f32, f32),
    scale: f32,
    pan: (f32, f32),
    fitted: bool,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            natural: (0.0, 0.0),
            viewport: (0.0, 0.0),
            scale: 1.0,
            pan: (0.0, 0.0),
            fitted: true,
        }
    }
}

impl Geometry {
    fn ready(&self) -> bool {
        self.natural.0 > 0.0
            && self.natural.1 > 0.0
            && self.viewport.0 > 0.0
            && self.viewport.1 > 0.0
    }

    /// The fit scale `min(vw / nw, vh / nh, 1)`; an image is never enlarged to fit. 1 while the
    /// sizes are unknown.
    pub fn fit(&self) -> f32 {
        if !self.ready() {
            return 1.0;
        }
        (self.viewport.0 / self.natural.0)
            .min(self.viewport.1 / self.natural.1)
            .min(1.0)
    }

    fn pan_limit(&self) -> (f32, f32) {
        let axis = |n: f32, v: f32| ((n * self.scale - v) / 2.0).max(0.0);
        (
            axis(self.natural.0, self.viewport.0),
            axis(self.natural.1, self.viewport.1),
        )
    }

    fn clamp_pan(&mut self) {
        let (lx, ly) = self.pan_limit();
        self.pan.0 = self.pan.0.clamp(-lx, lx);
        self.pan.1 = self.pan.1.clamp(-ly, ly);
    }

    fn refit(&mut self) {
        if self.fitted {
            self.scale = self.fit();
            self.pan = (0.0, 0.0);
        } else {
            self.clamp_pan();
        }
    }

    fn resize(&mut self, natural: Option<Size<Pixels>>, viewport: Option<Size<Pixels>>) {
        let mut changed = false;
        if let Some(n) = natural.filter(|s| valid_size(*s)) {
            let n = (f32::from(n.width), f32::from(n.height));
            changed |= n != self.natural;
            self.natural = n;
        }
        if let Some(v) = viewport.filter(|s| valid_size(*s)) {
            let v = (f32::from(v.width), f32::from(v.height));
            changed |= v != self.viewport;
            self.viewport = v;
        }
        if changed {
            self.refit();
        }
    }

    fn max_scale(&self) -> f32 {
        let natural_max = self.natural.0.max(self.natural.1);
        (LAYOUT_BOUND / natural_max).min(MAX_ZOOM).max(self.fit())
    }

    fn zoom(&mut self, scale: f32, anchor: Point<Pixels>) {
        let anchor = (f32::from(anchor.x), f32::from(anchor.y));
        if !scale.is_finite() || scale <= 0.0 || !anchor.0.is_finite() || !anchor.1.is_finite() {
            return;
        }
        if !self.ready() {
            return;
        }
        let fit = self.fit();
        let new_scale = scale.clamp(fit.min(MIN_ZOOM), self.max_scale());
        let ratio = new_scale / self.scale;
        let a = (
            anchor.0 - self.viewport.0 / 2.0,
            anchor.1 - self.viewport.1 / 2.0,
        );
        self.pan = (
            a.0 - (a.0 - self.pan.0) * ratio,
            a.1 - (a.1 - self.pan.1) * ratio,
        );
        self.scale = new_scale;
        self.fitted = false;
        self.clamp_pan();
    }

    fn pan_by(&mut self, delta: (f32, f32)) -> bool {
        if !delta.0.is_finite() || !delta.1.is_finite() {
            return false;
        }
        let before = self.pan;
        self.pan = (self.pan.0 + delta.0, self.pan.1 + delta.1);
        self.clamp_pan();
        self.pan != before
    }

    fn origin(&self) -> (f32, f32) {
        (
            (self.viewport.0 - self.natural.0 * self.scale) / 2.0 + self.pan.0,
            (self.viewport.1 - self.natural.1 * self.scale) / 2.0 + self.pan.1,
        )
    }
}

struct Press {
    pointer: Point<Pixels>,
    pan: (f32, f32),
}

#[derive(Default)]
struct State {
    geometry: Geometry,
    pinch: Option<(f32, f32)>,
    press: Option<Press>,
    dragged: bool,
    /// Window-space origin of the viewport, captured during prepaint.
    origin: Point<Pixels>,
    /// The view that owns the viewer; zoom, pan and drag notify only this view.
    owner: Option<EntityId>,
}

static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

/// A cloneable handle onto one viewer's state.
#[derive(Clone)]
pub struct ImageView {
    id: usize,
    state: Rc<RefCell<State>>,
}

impl Default for ImageView {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageView {
    /// A viewer in the fitted state with unknown sizes.
    pub fn new() -> Self {
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            state: Rc::new(RefCell::new(State::default())),
        }
    }

    /// Restores every geometry and drag value to its default: fitted, no pan, no press.
    pub fn reset(&self) {
        let mut state = self.state.borrow_mut();
        let owner = state.owner;
        let origin = state.origin;
        *state = State::default();
        state.owner = owner;
        state.origin = origin;
    }

    /// Starts a fresh click-versus-drag decision: clears the dragged flag and the press record.
    pub fn begin_click(&self) {
        let mut state = self.state.borrow_mut();
        state.dragged = false;
        state.press = None;
    }

    /// Whether the last press travelled far enough to count as a drag.
    pub fn dragged(&self) -> bool {
        self.state.borrow().dragged
    }

    /// Sets the natural size and the viewport (R1, R2). A size that is not finite or not
    /// greater than zero is ignored. A fitted viewer refits; otherwise the scale is kept and
    /// the pan re-clamped.
    pub fn resize(&self, natural: Size<Pixels>, viewport: Size<Pixels>) {
        self.state
            .borrow_mut()
            .geometry
            .resize(Some(natural), Some(viewport));
    }

    /// Sets only the natural size (see [`ImageView::resize`]).
    pub fn set_natural_size(&self, natural: Size<Pixels>) {
        self.state.borrow_mut().geometry.resize(Some(natural), None);
    }

    /// Sets only the viewport (see [`ImageView::resize`]).
    pub fn set_viewport(&self, viewport: Size<Pixels>) {
        self.state
            .borrow_mut()
            .geometry
            .resize(None, Some(viewport));
    }

    /// The fit scale `min(vw / nw, vh / nh, 1)`.
    pub fn fit(&self) -> f32 {
        self.state.borrow().geometry.fit()
    }

    /// The current scale.
    pub fn scale(&self) -> f32 {
        self.state.borrow().geometry.scale
    }

    /// The current pan: the displacement of the image centre from the viewport centre.
    pub fn pan(&self) -> Point<Pixels> {
        let pan = self.state.borrow().geometry.pan;
        point(px(pan.0), px(pan.1))
    }

    /// Whether the viewer still follows the fit scale.
    pub fn is_fitted(&self) -> bool {
        self.state.borrow().geometry.fitted
    }

    /// The natural size.
    pub fn natural_size(&self) -> Size<Pixels> {
        let n = self.state.borrow().geometry.natural;
        size(px(n.0), px(n.1))
    }

    /// The viewport size.
    pub fn viewport_size(&self) -> Size<Pixels> {
        let v = self.state.borrow().geometry.viewport;
        size(px(v.0), px(v.1))
    }

    /// Zooms to `scale` keeping the source point under `anchor` (viewport coordinates) fixed
    /// (R3). Requests with a scale that is not finite and positive, or a non-finite anchor,
    /// are ignored.
    pub fn zoom(&self, scale: f32, anchor: Point<Pixels>) {
        self.state.borrow_mut().geometry.zoom(scale, anchor);
    }

    /// Pans by `delta` and clamps; returns whether the pan changed.
    pub fn pan_by(&self, delta: Point<Pixels>) -> bool {
        self.state
            .borrow_mut()
            .geometry
            .pan_by((f32::from(delta.x), f32::from(delta.y)))
    }

    /// Origin of the displayed image rectangle in viewport coordinates.
    pub fn image_origin(&self) -> Point<Pixels> {
        let origin = self.state.borrow().geometry.origin();
        point(px(origin.0), px(origin.1))
    }

    /// The displayed image rectangle in viewport coordinates.
    pub fn image_bounds(&self) -> Bounds<Pixels> {
        let state = self.state.borrow();
        let g = &state.geometry;
        let origin = g.origin();
        Bounds {
            origin: point(px(origin.0), px(origin.1)),
            size: size(px(g.natural.0 * g.scale), px(g.natural.1 * g.scale)),
        }
    }

    fn notify(&self, cx: &mut App) {
        if let Some(owner) = self.state.borrow().owner {
            cx.notify(owner);
        }
    }

    fn local(&self, position: Point<Pixels>) -> Point<Pixels> {
        position - self.state.borrow().origin
    }

    fn wheel(&self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut App) {
        let delta = event.delta.pixel_delta(px(LINE_PIXELS));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if event.modifiers.control {
            let factor = (dy * WHEEL_ZOOM_RATE).clamp(-2.0, 2.0).exp();
            let scale = self.scale() * factor;
            self.zoom(scale, self.local(event.position));
            self.notify(cx);
            window.prevent_default();
            cx.stop_propagation();
        } else if self.pan_by(point(px(dx), px(dy))) {
            self.notify(cx);
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    fn pinch(&self, event: &PinchEvent, cx: &mut App) {
        match event.phase {
            TouchPhase::Started => {
                let scale = self.scale();
                self.state.borrow_mut().pinch = Some((scale, 1.0));
            }
            TouchPhase::Ended | TouchPhase::Cancelled => {
                self.state.borrow_mut().pinch = None;
            }
            TouchPhase::Moved => {
                if !event.delta.is_finite() {
                    cx.stop_propagation();
                    return;
                }
                let target = {
                    let mut state = self.state.borrow_mut();
                    let current = state.geometry.scale;
                    let (start, factor) = state.pinch.unwrap_or((current, 1.0));
                    let factor = (factor + event.delta).max(PINCH_FACTOR_FLOOR);
                    state.pinch = Some((start, factor));
                    start * factor
                };
                self.zoom(target, self.local(event.position));
                self.notify(cx);
            }
        }
        cx.stop_propagation();
    }

    fn press(&self, event: &MouseDownEvent, window: &mut Window) {
        let mut state = self.state.borrow_mut();
        let pointer = event.position - state.origin;
        state.press = Some(Press {
            pointer,
            pan: state.geometry.pan,
        });
        state.dragged = false;
        drop(state);
        window.prevent_default();
    }

    fn pointer_moved(&self, event: &MouseMoveEvent, cx: &mut App) {
        let mut state = self.state.borrow_mut();
        let Some((press_pointer, press_pan)) = state.press.as_ref().map(|p| (p.pointer, p.pan))
        else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            state.press = None;
            return;
        }
        let delta = (event.position - state.origin) - press_pointer;
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if !state.dragged && dx.hypot(dy) < DRAG_THRESHOLD {
            return;
        }
        state.dragged = true;
        state.geometry.pan = (press_pan.0 + dx, press_pan.1 + dy);
        state.geometry.clamp_pan();
        drop(state);
        self.notify(cx);
    }

    fn release(&self) {
        self.state.borrow_mut().press = None;
    }

    fn click(
        &self,
        event: &ClickEvent,
        handler: Option<&ImageClickHandler>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.dragged() {
            cx.stop_propagation();
            return;
        }
        let inside = {
            let state = self.state.borrow();
            let local = event.position() - state.origin;
            let g = &state.geometry;
            let origin = g.origin();
            let (x, y) = (f32::from(local.x), f32::from(local.y));
            x >= origin.0
                && x <= origin.0 + g.natural.0 * g.scale
                && y >= origin.1
                && y <= origin.1 + g.natural.1 * g.scale
        };
        if inside && let Some(handler) = handler {
            handler(event, window, cx);
            cx.stop_propagation();
        }
    }

    /// The viewport element: it takes the remaining space of its parent flex container (minimum
    /// size zero), clips its children, paints `image` at exactly the displayed rectangle with
    /// contain fitting (over the optional backing `plate` fill) and handles wheel, pinch and
    /// drag. `natural_size` is the image's natural size in logical pixels; `on_image_click`
    /// receives a click that lands inside the displayed image.
    pub fn render(
        &self,
        image: Arc<RenderImage>,
        natural_size: Size<Pixels>,
        on_image_click: Option<ImageClickHandler>,
        plate: Option<Hsla>,
    ) -> impl IntoElement {
        self.set_natural_size(natural_size);
        let measure = self.clone();
        let paint = self.clone();
        let wheel = self.clone();
        let pinch = self.clone();
        let press = self.clone();
        let click = self.clone();
        div()
            .id(ElementId::from(("image-view", self.id)))
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, move |event, window, _| {
                press.press(event, window)
            })
            .on_scroll_wheel(move |event, window, cx| wheel.wheel(event, window, cx))
            .on_pinch(move |event, _, cx| pinch.pinch(event, cx))
            .on_click(move |event, window, cx| {
                click.click(event, on_image_click.as_ref(), window, cx)
            })
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let changed = {
                            let mut state = measure.state.borrow_mut();
                            state.origin = bounds.origin;
                            state.owner = Some(window.current_view());
                            let before = state.geometry.viewport;
                            state.geometry.resize(None, Some(bounds.size));
                            state.geometry.viewport != before
                        };
                        if changed {
                            let owner = window.current_view();
                            window.defer(cx, move |_, cx| cx.notify(owner));
                        }
                    },
                    move |bounds, (), window, _| {
                        let view = paint.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                view.pointer_moved(event, cx);
                            }
                        });
                        let view = paint.clone();
                        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, _| {
                            if phase == DispatchPhase::Capture {
                                view.release();
                            }
                        });
                        let rect = {
                            let state = paint.state.borrow();
                            let g = &state.geometry;
                            if !g.ready() {
                                return;
                            }
                            let origin = g.origin();
                            Bounds {
                                origin: bounds.origin + point(px(origin.0), px(origin.1)),
                                size: size(px(g.natural.0 * g.scale), px(g.natural.1 * g.scale)),
                            }
                        };
                        if let Some(plate) = plate {
                            window.paint_quad(vespertine::quad(
                                rect,
                                px(PLATE_RADIUS),
                                plate,
                                0.0,
                                vespertine::transparent_black(),
                                vespertine::BorderStyle::default(),
                            ));
                        }
                        let fitted = contain(rect, image.size(0));
                        let _ = window.paint_image(
                            rect,
                            fitted,
                            vespertine::Corners::default(),
                            image,
                            0,
                            false,
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

/// The largest rectangle with the image's aspect ratio centred in `rect`.
fn contain(rect: Bounds<Pixels>, image: Size<vespertine::DevicePixels>) -> Bounds<Pixels> {
    let (iw, ih) = (
        i32::from(image.width) as f32,
        i32::from(image.height) as f32,
    );
    if iw <= 0.0 || ih <= 0.0 {
        return rect;
    }
    let (rw, rh) = (f32::from(rect.size.width), f32::from(rect.size.height));
    let scale = (rw / iw).min(rh / ih);
    let (w, h) = (iw * scale, ih * scale);
    Bounds {
        origin: rect.origin + point(px((rw - w) / 2.0), px((rh - h) / 2.0)),
        size: size(px(w), px(h)),
    }
}
