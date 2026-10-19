//! The retained, interactive 3-D image surface (CMP-025).
//!
//! [`SceneSurface`] is an ordinary element of a caller-chosen height and the available width. It
//! shows the newest completed frame of an external [`SceneProducer`] as an image painted exactly
//! into its bounds, owns the camera and the pointer interaction, schedules at most one frame in
//! flight off the foreground thread and keeps result identity: a result is displayed only if the
//! surface still exists with the identity the request was made for and no newer frame is already
//! shown ([`FrameCore`]).
//!
//! State lives in a per-surface record keyed by the surface key. It survives re-renders and
//! scene changes; it is torn down when the surface stops being painted (the element state that
//! owns it is dropped by the framework), when its identity changes, or explicitly through
//! [`SceneSurfaces`]. Teardown releases the displayed image from every window's atlas through the
//! image-eviction path (APP-001), clears retained producer targets and makes late results
//! discard and release themselves.

use super::camera::{Camera, OrbitController, wheel_notches};
use super::core::{Delivery, FrameCore};
use super::quality::{Quality, RendererKind, select_quality};
use super::types::{
    Annotation, InputContext, Label, Palette, RenderKey, RenderRequest, RenderResult, SceneFrame,
    TargetSlot, hovered_annotation, pixel_size,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vespertine::{
    AnyElement, App, AsyncApp, Bounds, Corners, CursorStyle, DispatchPhase, Element, ElementId,
    EntityId, FontWeight, ForegroundExecutor, Global, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PinchEvent, Pixels, Point, RenderImage, ScrollWheelEvent, Style,
    Styled, Window, WindowId, div, px, relative,
};

/// Smallest surface height.
pub const MIN_HEIGHT: f32 = 40.0;
/// Largest surface height.
pub const MAX_HEIGHT: f32 = 720.0;
/// Height used when the host chooses none.
pub const DEFAULT_HEIGHT: f32 = 320.0;

/// The external producer of scene frames.
///
/// `render` runs off the foreground thread (or wherever the host runs it, see [`Dispatch`]) and
/// must return straight-alpha BGRA pixels, top row first, of the key's pixel size.
pub trait SceneProducer: Send + Sync + 'static {
    /// Renders the frame of `request`.
    fn render(&self, request: &RenderRequest) -> Result<SceneFrame, String>;

    /// Receives the message of a failed render (on the foreground thread). The last good image
    /// stays visible.
    fn report_error(&self, _message: &str) {}

    /// Asked on every paint of an animating surface whether the scene has a new drawing at
    /// `time` seconds since the surface was created; answering `true` bumps the scene
    /// generation. Static scenes are never asked.
    fn new_drawing(&self, _time: f32) -> bool {
        true
    }

    /// A hash of the inputs besides camera, size, scale, quality, palette and generation that
    /// the output depends on (pointer, time, uniforms). The surface re-renders when it differs
    /// from the displayed frame's. Zero (the default) means there are none.
    fn input_key(&self, _context: &InputContext) -> u64 {
        0
    }

    /// Whether the producer needs an animation frame at every paint while motion is permitted,
    /// besides what the surface's own animate flag and turntable ask for.
    fn animating(&self, _reduce_motion: bool) -> bool {
        false
    }
}

/// A producer made from a render function.
pub struct FnProducer<F>(pub F);

impl<F> SceneProducer for FnProducer<F>
where
    F: Fn(&RenderRequest) -> Result<SceneFrame, String> + Send + Sync + 'static,
{
    fn render(&self, request: &RenderRequest) -> Result<SceneFrame, String> {
        (self.0)(request)
    }
}

/// Where requests run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Dispatch {
    /// On the background executor; the result is delivered on the foreground when it completes.
    #[default]
    Background,
    /// Requests are only queued (see [`SurfaceHandle::take_requests`]); the host renders them
    /// and delivers the results with [`SurfaceHandle::deliver`], in any order.
    Manual,
}

/// Defers the release of images to a point where the window being updated is back in the
/// application, from contexts that have no application access (a dropped element state).
#[derive(Clone)]
pub(crate) struct Releaser {
    executor: ForegroundExecutor,
    app: AsyncApp,
}

impl Releaser {
    pub(crate) fn new(cx: &App) -> Self {
        Self {
            executor: cx.foreground_executor().clone(),
            app: cx.to_async(),
        }
    }

    pub(crate) fn release(&self, images: Vec<Arc<RenderImage>>) {
        if images.is_empty() {
            return;
        }
        let app = self.app.clone();
        self.executor
            .spawn(async move {
                app.update(|cx| {
                    for image in images {
                        cx.drop_image(image, None);
                    }
                })
            })
            .detach();
    }
}

/// Configuration applied from the element every frame.
#[derive(Clone)]
struct Config {
    animate: bool,
    quality_interacting: Quality,
    quality_settled: Quality,
    renderer: Option<RendererKind>,
    dispatch: Dispatch,
    max_in_flight: usize,
    palette: Option<Palette>,
}

pub(crate) struct SceneState {
    core: FrameCore<RenderKey, SceneFrame>,
    controller: OrbitController,
    producer: Arc<dyn SceneProducer>,
    config: Config,
    generation: u64,
    created: Instant,
    pointer: Option<Point<Pixels>>,
    hovered: Option<usize>,
    origin: Point<Pixels>,
    size: vespertine::Size<Pixels>,
    last_error: Option<String>,
    last_settled_cost: Option<Duration>,
    last_render_time: Option<Duration>,
    pending: Vec<RenderRequest>,
    press_origin: Option<Point<Pixels>>,
    track_pointer: bool,
    failed_key: Option<RenderKey>,
    last_requested: Option<RenderKey>,
    epoch: u64,
    user_identity: u64,
    animation_frames: u64,
    owner: Option<EntityId>,
    targets: TargetSlot,
    releaser: Releaser,
}

impl SceneState {
    fn notify(&self, cx: &mut App) {
        if let Some(owner) = self.owner {
            cx.notify(owner);
        }
    }

    fn recompute_hover(&mut self) -> bool {
        let hovered = self
            .core
            .displayed()
            .and_then(|d| hovered_annotation(&d.payload.annotations, self.pointer));
        let changed = hovered != self.hovered;
        self.hovered = hovered;
        changed
    }

    /// A replacement that keeps the displayed frame: results of requests made so far are
    /// discarded on arrival, retained targets are dropped and nothing is outstanding.
    fn soft_replace(&mut self) {
        let next = self.core.identity() + 1;
        self.core.reidentify(next);
        self.targets.clear();
        self.pending.clear();
        self.failed_key = None;
        self.last_requested = None;
    }

    /// Releases everything the surface holds and marks it removed.
    fn teardown(&mut self) {
        let image = self.core.remove().map(|frame| frame.image);
        self.targets.clear();
        self.pending.clear();
        self.releaser.release(image.into_iter().collect());
    }
}

/// A handle onto the retained state of one surface: the observable state (camera, displayed
/// key, in-flight count, last error) and the delivery hooks.
#[derive(Clone)]
pub struct SurfaceHandle(Rc<RefCell<SceneState>>);

impl SurfaceHandle {
    /// The camera displayed at `now`: the stored camera plus the turntable yaw.
    pub fn camera_at(&self, now: Instant, reduce_motion: bool) -> Camera {
        self.0.borrow().controller.displayed(now, reduce_motion)
    }

    /// The stored camera (without the turntable).
    pub fn camera(&self) -> Camera {
        self.0.borrow().controller.stored()
    }

    /// Sets the stored camera (clamped). Changing the camera never replaces or resets the
    /// retained scene: the shown image stays until a frame for the new camera lands.
    pub fn set_camera(&self, camera: Camera, cx: &mut App) {
        let mut state = self.0.borrow_mut();
        state.controller.set_camera(camera, Instant::now());
        state.notify(cx);
    }

    /// The key the displayed frame was rendered for.
    pub fn displayed_key(&self) -> Option<RenderKey> {
        self.0.borrow().core.displayed_key().copied()
    }

    /// The sequence number of the displayed frame.
    pub fn displayed_sequence(&self) -> Option<u64> {
        self.0.borrow().core.displayed().map(|d| d.sequence)
    }

    /// The displayed image.
    pub fn displayed_image(&self) -> Option<Arc<RenderImage>> {
        self.0
            .borrow()
            .core
            .displayed()
            .map(|d| d.payload.image.clone())
    }

    /// Frames in flight.
    pub fn in_flight(&self) -> usize {
        self.0.borrow().core.in_flight()
    }

    /// The last producer error; cleared by the next successful frame.
    pub fn last_error(&self) -> Option<String> {
        self.0.borrow().last_error.clone()
    }

    /// How long the producer took for the displayed frame.
    pub fn last_render_time(&self) -> Option<Duration> {
        self.0.borrow().last_render_time
    }

    /// The scene generation.
    pub fn generation(&self) -> u64 {
        self.0.borrow().generation
    }

    /// Bumps the scene generation: the producer supplies a new drawing. The shown image and any
    /// retained targets stay until the new drawing's first frame lands.
    pub fn bump_generation(&self, cx: &mut App) {
        let mut state = self.0.borrow_mut();
        state.generation += 1;
        state.notify(cx);
    }

    /// The hovered annotation of the displayed frame, if any.
    pub fn hovered(&self) -> Option<Annotation> {
        let state = self.0.borrow();
        let index = state.hovered?;
        state
            .core
            .displayed()
            .and_then(|d| d.payload.annotations.get(index).cloned())
    }

    /// Where the primary button was pressed while it is held (tracked when the surface orbits
    /// or tracks the pointer).
    pub fn press_origin(&self) -> Option<Point<Pixels>> {
        self.0.borrow().press_origin
    }

    /// The pointer position relative to the surface origin while it is over the surface.
    pub fn pointer(&self) -> Option<Point<Pixels>> {
        self.0.borrow().pointer
    }

    /// The labels of the displayed frame.
    pub fn labels(&self) -> Vec<Label> {
        self.0
            .borrow()
            .core
            .displayed()
            .map(|d| d.payload.labels.clone())
            .unwrap_or_default()
    }

    /// How many animation frames the surface asked for.
    pub fn animation_frames(&self) -> u64 {
        self.0.borrow().animation_frames
    }

    /// Whether the surface was removed.
    pub fn is_removed(&self) -> bool {
        self.0.borrow().core.is_removed()
    }

    /// Takes the queued requests of a [`Dispatch::Manual`] surface.
    pub fn take_requests(&self) -> Vec<RenderRequest> {
        std::mem::take(&mut self.0.borrow_mut().pending)
    }

    /// Delivers a result. It becomes the displayed frame unless the surface was removed or
    /// replaced, or a frame with a sequence number at least as new is displayed; a dropped
    /// result's image is released from every window's atlas. Returns the outcome.
    pub fn deliver(&self, result: RenderResult, cx: &mut App) -> Delivery<SceneFrame> {
        deliver(&self.0, result, cx)
    }

    /// Explicit resource drop: releases the displayed image and the retained producer targets.
    /// The surface keeps working and renders again at the next paint.
    pub fn release(&self, cx: &mut App) {
        let mut state = self.0.borrow_mut();
        let image = state.core.take_displayed().map(|frame| frame.image);
        state.targets.clear();
        state.hovered = None;
        state.last_requested = None;
        let releaser = state.releaser.clone();
        state.notify(cx);
        drop(state);
        releaser.release(image.into_iter().collect());
    }

    /// Removal handling: tears the surface down as if it left the tree.
    pub fn remove(&self) {
        self.0.borrow_mut().teardown();
    }

    /// The retained producer targets of this surface.
    pub fn targets(&self) -> TargetSlot {
        self.0.borrow().targets.clone()
    }
}

fn deliver(
    state: &Rc<RefCell<SceneState>>,
    result: RenderResult,
    cx: &mut App,
) -> Delivery<SceneFrame> {
    let mut guard = state.borrow_mut();
    let releaser = guard.releaser.clone();
    match result.outcome {
        Ok(frame) => {
            let settled = result.key.quality == guard.config.quality_settled;
            let delivery = guard
                .core
                .complete(result.identity, result.sequence, result.key, frame);
            match &delivery {
                Delivery::Displayed { replaced } => {
                    guard.last_render_time = Some(result.render_time);
                    guard.last_error = None;
                    guard.failed_key = None;
                    if settled {
                        guard.last_settled_cost = Some(result.render_time);
                    }
                    guard.recompute_hover();
                    guard.notify(cx);
                    if let Some(old) = replaced {
                        releaser.release(vec![old.image.clone()]);
                    }
                }
                Delivery::Discarded { payload, .. } => {
                    if guard.last_requested == Some(result.key) {
                        guard.last_requested = None;
                    }
                    releaser.release(vec![payload.image.clone()]);
                }
            }
            delivery
        }
        Err(message) => {
            if guard.core.complete_without_frame(result.identity) {
                guard.failed_key = Some(result.key);
                guard.last_error = Some(message.clone());
                let producer = guard.producer.clone();
                guard.notify(cx);
                drop(guard);
                producer.report_error(&message);
            }
            Delivery::Displayed { replaced: None }
        }
    }
}

/// The per-application index of the live surfaces, for hosts and tests.
#[derive(Default)]
pub struct SceneSurfaces {
    map: HashMap<(WindowId, ElementId), Weak<RefCell<SceneState>>>,
}

impl Global for SceneSurfaces {}

impl SceneSurfaces {
    /// The surface with `key` in `window`, if it exists.
    pub fn get(cx: &App, window: WindowId, key: &ElementId) -> Option<SurfaceHandle> {
        cx.try_global::<SceneSurfaces>()?
            .map
            .get(&(window, key.clone()))?
            .upgrade()
            .map(SurfaceHandle)
    }

    /// Tree-update hook: tears down and forgets every surface of `window` whose key is not in
    /// `live`.
    pub fn retain_live(cx: &mut App, window: WindowId, live: &[ElementId]) {
        let Some(registry) = cx.try_global::<SceneSurfaces>() else {
            return;
        };
        let stale: Vec<_> = registry
            .map
            .keys()
            .filter(|(w, key)| *w == window && !live.contains(key))
            .cloned()
            .collect();
        for entry in stale {
            if let Some(state) = cx
                .global_mut::<SceneSurfaces>()
                .map
                .remove(&entry)
                .and_then(|weak| weak.upgrade())
            {
                state.borrow_mut().teardown();
            }
        }
    }
}

/// Owns a surface's state for as long as the element is painted: when the framework drops the
/// element state, the surface is torn down.
struct Lease(Rc<RefCell<SceneState>>);

impl Drop for Lease {
    fn drop(&mut self) {
        self.0.borrow_mut().teardown();
    }
}

/// A retained, interactive 3-D image surface. See the module documentation.
pub struct SceneSurface {
    key: ElementId,
    producer: Arc<dyn SceneProducer>,
    height: Pixels,
    orbit: bool,
    spin: f32,
    camera: Camera,
    zoom_step: f32,
    identity: u64,
    epoch: u64,
    track_pointer: bool,
    config: Config,
}

impl SceneSurface {
    /// A surface keyed by `key` (stable across frames, unique within its window) that shows the
    /// frames of `producer`.
    pub fn new(key: impl Into<ElementId>, producer: impl SceneProducer) -> Self {
        Self::from_producer(key, Arc::new(producer))
    }

    /// Like [`SceneSurface::new`] for a shared producer.
    pub fn from_producer(key: impl Into<ElementId>, producer: Arc<dyn SceneProducer>) -> Self {
        Self {
            key: key.into(),
            producer,
            height: px(DEFAULT_HEIGHT),
            orbit: true,
            spin: 0.0,
            camera: Camera::default(),
            zoom_step: super::camera::DEFAULT_ZOOM_STEP,
            identity: 0,
            epoch: 0,
            track_pointer: false,
            config: Config {
                animate: false,
                quality_interacting: Quality::Draft,
                quality_settled: Quality::Supersampled,
                renderer: None,
                dispatch: Dispatch::Background,
                max_in_flight: 1,
                palette: None,
            },
        }
    }

    /// Logical height, clamped to 40 to 720.
    pub fn height(mut self, height: impl Into<Pixels>) -> Self {
        let height = f32::from(height.into());
        self.height = px(if height.is_finite() {
            height.clamp(MIN_HEIGHT, MAX_HEIGHT)
        } else {
            DEFAULT_HEIGHT
        });
        self
    }

    /// Enables or disables orbit dragging (default enabled).
    pub fn orbit(mut self, orbit: bool) -> Self {
        self.orbit = orbit;
        self
    }

    /// Turntable in radians per second, clamped to [-3, 3] (default 0).
    pub fn spin(mut self, spin: f32) -> Self {
        self.spin = spin;
        self
    }

    /// The initial camera; the retained camera survives re-renders and scene changes.
    pub fn camera(mut self, yaw: f32, pitch: f32, zoom: f32) -> Self {
        self.camera = Camera::new(yaw, pitch, zoom);
        self
    }

    /// Whether the scene is time-varying: an animating surface asks for an animation frame and a
    /// new drawing at every paint.
    pub fn animate(mut self, animate: bool) -> Self {
        self.config.animate = animate;
        self
    }

    /// Quality while interacting (turntable running or dragging); default Draft.
    pub fn quality_interacting(mut self, quality: Quality) -> Self {
        self.config.quality_interacting = quality;
        self
    }

    /// Quality when settled; default 2 x 2 supersampling.
    pub fn quality_settled(mut self, quality: Quality) -> Self {
        self.config.quality_settled = quality;
        self
    }

    /// Zoom factor per wheel notch (default 1.1).
    pub fn zoom_step(mut self, step: f32) -> Self {
        self.zoom_step = step;
        self
    }

    /// The kind of renderer behind the producer; by default a GPU renderer is assumed when the
    /// window has a GPU context and a software renderer otherwise.
    pub fn renderer(mut self, renderer: RendererKind) -> Self {
        self.config.renderer = Some(renderer);
        self
    }

    /// Where requests run (default: the background executor).
    pub fn dispatch(mut self, dispatch: Dispatch) -> Self {
        self.config.dispatch = dispatch;
        self
    }

    /// How many frames may be in flight (default 1).
    pub fn max_in_flight(mut self, limit: usize) -> Self {
        self.config.max_in_flight = limit.max(1);
        self
    }

    /// Overrides the palette (default: the palette of the active theme). A change of palette
    /// changes the palette identity in the key and so produces a re-render without discarding
    /// the camera.
    pub fn palette(mut self, palette: Palette) -> Self {
        self.config.palette = Some(palette);
        self
    }

    /// Records the press origin of the primary button and captures the pointer for the press
    /// even when orbit is disabled (surfaces whose output depends on the mouse).
    pub fn track_pointer(mut self, track: bool) -> Self {
        self.track_pointer = track;
        self
    }

    /// A replacement epoch. Changing it under the same key and identity replaces the surface
    /// in place: retained targets are dropped and results of earlier requests are discarded and
    /// released, but the displayed frame stays visible until the first frame of the new epoch
    /// lands.
    pub fn epoch(mut self, epoch: u64) -> Self {
        self.epoch = epoch;
        self
    }

    /// The identity of this surface. A different identity under the same key replaces the
    /// surface: the displayed image is released, camera and pointer state are dropped and
    /// results of the old identity are discarded.
    pub fn identity(mut self, identity: u64) -> Self {
        self.identity = identity;
        self
    }
}

impl IntoElement for SceneSurface {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

/// Layout state of a [`SceneSurface`].
pub struct SurfaceLayout {
    state: Rc<RefCell<SceneState>>,
    overlay: AnyElement,
}

/// Prepaint state of a [`SceneSurface`].
pub struct SurfacePrepaint {
    hitbox: Hitbox,
}

impl SceneSurface {
    fn lease(
        &mut self,
        global_id: &GlobalElementId,
        window: &mut Window,
        cx: &mut App,
    ) -> Rc<RefCell<SceneState>> {
        let identity = self.identity;
        let producer = self.producer.clone();
        let (camera, orbit, spin, zoom_step, config) = (
            self.camera,
            self.orbit,
            self.spin,
            self.zoom_step,
            self.config.clone(),
        );
        let releaser = Releaser::new(cx);
        let state = window.with_element_state(global_id, |lease: Option<Lease>, _| {
            let existing = lease.filter(|lease| lease.0.borrow().user_identity == identity);
            let lease = existing.unwrap_or_else(|| {
                let now = Instant::now();
                Lease(Rc::new(RefCell::new(SceneState {
                    core: FrameCore::new(identity),
                    controller: OrbitController::new(camera, now),
                    producer: producer.clone(),
                    config: config.clone(),
                    generation: 0,
                    created: now,
                    pointer: None,
                    hovered: None,
                    origin: Point::default(),
                    size: Default::default(),
                    last_error: None,
                    last_settled_cost: None,
                    last_render_time: None,
                    pending: Vec::new(),
                    press_origin: None,
                    track_pointer: false,
                    failed_key: None,
                    last_requested: None,
                    epoch: self.epoch,
                    user_identity: identity,
                    animation_frames: 0,
                    owner: None,
                    targets: TargetSlot::default(),
                    releaser: releaser.clone(),
                })))
            });
            (lease.0.clone(), lease)
        });
        {
            let mut guard = state.borrow_mut();
            guard.producer = self.producer.clone();
            guard.config = self.config.clone();
            guard.core.set_limit(self.config.max_in_flight);
            guard.controller.set_orbit(orbit);
            guard.controller.set_zoom_step(zoom_step);
            guard.controller.set_spin(spin, Instant::now());
            guard.track_pointer = self.track_pointer;
            if guard.epoch != self.epoch {
                guard.epoch = self.epoch;
                guard.soft_replace();
            }
        }
        state
    }
}

fn overlay_element(state: &SceneState, palette: &Palette) -> AnyElement {
    let height = f32::from(state.size.height);
    let mut overlay = div().absolute().size_full();
    if let Some(displayed) = state.core.displayed() {
        for label in &displayed.payload.labels {
            let (x, y) = (f32::from(label.position.x), f32::from(label.position.y));
            overlay = overlay.child(
                div()
                    .absolute()
                    .left(px(x - 150.))
                    .w(px(300.))
                    .bottom(px(height - y))
                    .flex()
                    .justify_center()
                    .text_size(px(label.size.pixels()))
                    .text_color(palette.color(label.color))
                    .child(label.text.clone()),
            );
        }
        if let Some(annotation) = state
            .hovered
            .and_then(|index| displayed.payload.annotations.get(index))
        {
            overlay = overlay.child(
                div()
                    .absolute()
                    .top(px(6.))
                    .left(px(8.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.text)
                    .child(annotation.text.clone()),
            );
        }
    }
    overlay.into_any_element()
}

impl Element for SceneSurface {
    type RequestLayoutState = SurfaceLayout;
    type PrepaintState = SurfacePrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(self.key.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, SurfaceLayout) {
        let global_id = global_id.expect("a keyed element has a global id");
        let state = self.lease(global_id, window, cx);
        cx.default_global::<SceneSurfaces>().map.insert(
            (window.window_handle().window_id(), self.key.clone()),
            Rc::downgrade(&state),
        );
        let palette = self.config.palette.unwrap_or_else(|| Palette::current(cx));
        state.borrow_mut().size.height = self.height;
        let mut overlay = overlay_element(&state.borrow(), &palette);
        let overlay_layout = overlay.request_layout(window, cx);
        let mut style = Style::default();
        style.position = vespertine::Position::Relative;
        style.size.width = relative(1.).into();
        style.size.height = self.height.into();
        let layout = window.request_layout(style, [overlay_layout], cx);
        (layout, SurfaceLayout { state, overlay })
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut SurfaceLayout,
        window: &mut Window,
        cx: &mut App,
    ) -> SurfacePrepaint {
        {
            let mut state = layout.state.borrow_mut();
            state.origin = bounds.origin;
            state.size = bounds.size;
            state.owner = Some(window.current_view());
        }
        layout.overlay.prepaint(window, cx);
        SurfacePrepaint {
            hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal),
        }
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut SurfaceLayout,
        prepaint: &mut SurfacePrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let state = layout.state.clone();
        let hitbox = prepaint.hitbox.clone();
        if let Some(image) = state
            .borrow()
            .core
            .displayed()
            .map(|d| d.payload.image.clone())
        {
            let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
        }
        layout.overlay.paint(window, cx);
        register_input(&state, &hitbox, window);
        schedule(&state, bounds, window, cx);
    }
}

fn register_input(state: &Rc<RefCell<SceneState>>, hitbox: &Hitbox, window: &mut Window) {
    // Hitbox ids are frame-local: a press that is still held follows the element to this
    // frame's hitbox so the capture survives redraws.
    {
        let guard = state.borrow();
        if guard.controller.dragging() || guard.press_origin.is_some() {
            window.capture_pointer(hitbox.id);
        }
    }
    if state.borrow().controller.orbit() {
        let style = if state.borrow().controller.dragging() {
            CursorStyle::ClosedHand
        } else {
            CursorStyle::OpenHand
        };
        window.set_cursor_style(style, hitbox);
    }

    {
        let (state, hitbox) = (state.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Left
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let mut guard = state.borrow_mut();
            let local = event.position - guard.origin;
            let orbiting = guard
                .controller
                .press(local, Instant::now(), cx.reduce_motion());
            if orbiting || guard.track_pointer {
                guard.press_origin = Some(local);
                window.capture_pointer(hitbox.id);
                if orbiting {
                    cx.stop_propagation();
                }
                guard.notify(cx);
            }
        });
    }
    {
        let (state, hitbox) = (state.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let mut guard = state.borrow_mut();
            let local = event.position - guard.origin;
            let mut changed = guard.controller.drag_to(local);
            let pointer = hitbox.is_hovered(window).then_some(local);
            if pointer != guard.pointer {
                guard.pointer = pointer;
                changed |= guard.recompute_hover();
            }
            if changed {
                guard.notify(cx);
            }
        });
    }
    {
        let state = state.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                let mut guard = state.borrow_mut();
                let released = guard.controller.release(Instant::now());
                if released || guard.press_origin.take().is_some() {
                    guard.press_origin = None;
                    guard.notify(cx);
                }
            }
        });
    }
    {
        let (state, hitbox) = (state.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.should_handle_scroll(window) {
                return;
            }
            let mut guard = state.borrow_mut();
            guard.controller.wheel(wheel_notches(event.delta));
            guard.notify(cx);
            window.prevent_default();
            cx.stop_propagation();
        });
    }
    {
        let (state, hitbox) = (state.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &PinchEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !hitbox.is_hovered(window) {
                return;
            }
            let mut guard = state.borrow_mut();
            guard.controller.pinch(event.phase, event.delta);
            guard.notify(cx);
            cx.stop_propagation();
        });
    }
}

/// Animation and request scheduling, run at every paint.
fn schedule(
    state: &Rc<RefCell<SceneState>>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let reduce = cx.reduce_motion();
    let now = Instant::now();
    let mut guard = state.borrow_mut();
    let palette = guard.config.palette.unwrap_or_else(|| Palette::current(cx));
    guard.owner = Some(window.current_view());

    let time = now.saturating_duration_since(guard.created).as_secs_f32();
    let animate = guard.config.animate && !reduce;
    let turntable = guard.controller.turntable_running(reduce);
    if animate || turntable || guard.producer.animating(reduce) {
        window.request_animation_frame();
        guard.animation_frames += 1;
    }
    if animate
        && guard.core.in_flight() < guard.config.max_in_flight
        && guard.producer.new_drawing(time)
    {
        guard.generation += 1;
    }

    let (pw, ph) = pixel_size(bounds.size, scale);
    if pw == 0 || ph == 0 {
        return;
    }
    let camera = guard.controller.displayed(now, reduce);
    let renderer = guard.config.renderer.unwrap_or_else(|| {
        if window.gpu_context().is_some() {
            RendererKind::Gpu
        } else {
            RendererKind::Software
        }
    });
    let quality = select_quality(
        renderer,
        guard.controller.dragging() || turntable,
        guard.last_settled_cost,
        guard.config.quality_interacting,
        guard.config.quality_settled,
    );
    let input = guard.producer.input_key(&InputContext {
        pointer: guard.pointer,
        press: guard.press_origin,
        time,
        reduce_motion: reduce,
        size: bounds.size,
        scale,
    });
    let key = RenderKey {
        yaw: camera.yaw,
        pitch: camera.pitch,
        zoom: camera.zoom,
        size: bounds.size,
        scale,
        quality,
        palette: palette.id(),
        generation: guard.generation,
        input,
    };
    // A key that just failed is not retried until something changes.
    if guard.failed_key == Some(key)
        || guard.last_requested == Some(key)
        || !guard.core.needs_request(&key)
    {
        return;
    }
    guard.last_requested = Some(key);
    let sequence = guard.core.begin();
    let request = RenderRequest {
        key,
        sequence,
        identity: guard.core.identity(),
        time,
        palette,
        reduce_motion: reduce,
        pointer: guard.pointer,
        press: guard.press_origin,
        targets: guard.targets.clone(),
    };
    match guard.config.dispatch {
        Dispatch::Manual => guard.pending.push(request),
        Dispatch::Background => {
            let producer = guard.producer.clone();
            let weak = Rc::downgrade(state);
            drop(guard);
            let task = cx.background_executor().spawn({
                let request = request.clone();
                async move {
                    let started = Instant::now();
                    let outcome = producer.render(&request);
                    (outcome, started.elapsed())
                }
            });
            cx.spawn(async move |cx| {
                let (outcome, render_time) = task.await;
                cx.update(|cx| {
                    let result = RenderResult {
                        sequence: request.sequence,
                        key: request.key,
                        identity: request.identity,
                        outcome,
                        render_time,
                    };
                    match weak.upgrade() {
                        Some(state) => {
                            deliver(&state, result, cx);
                        }
                        None => {
                            if let Ok(frame) = result.outcome {
                                let image = frame.image;
                                cx.defer(move |cx| cx.drop_image(image, None));
                            }
                        }
                    }
                });
            })
            .detach();
        }
    }
}
