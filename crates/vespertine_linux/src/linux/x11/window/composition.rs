//! The window side of scene layering around native child content on X11 (PLT-001 R1, R5-R8,
//! R10-R18, R21-R25): enabling, native child attachments, frame submission with the overlay
//! window and the shape cutouts, the generation, focus and window destruction.
//!
//! The X requests themselves live in `x11::composition`; this module owns the state they act on
//! (the renderers, the attachments, the cached mode decision) and orders them within a frame:
//!
//! 1. attachment geometry and visibility were queued by the presentation callbacks (nothing is
//!    flushed by them);
//! 2. shapes, the overlay window's geometry and shape, its restack above the window are queued;
//! 3. one flush sends the whole batch;
//! 4. the overlay window is drawn, and last the window's own scene.

use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
};

use anyhow::{Context as _, Result, bail, ensure};
use vespertine::{
    Bounds, ComposedScene, CompositionLayout, DevicePixels, InputForwarding, NativeAttachment,
    NativeComposition, NativeDevice, NativeLayer, OcclusionTracker, Pixels, Point, Scene, Size,
    SurfaceId, SurfaceKind, X11CompositingFacts, X11CompositionMode, decide_x11_composition_mode,
    native_cutout, overlay_window_needed,
};
use vespertine_collections::HashMap;
use vespertine_util::ResultExt as _;
use vespertine_wgpu::{GpuContext, WgpuRenderer, WgpuSurfaceConfig, wgpu};
use x11rb::{
    connection::Connection,
    protocol::xproto::{self, ConnectionExt as _},
    xcb_ffi::XCBConnection,
};

use super::{RawWindow, X11Window, X11WindowState, X11WindowStatePtr, get_reply, xcb_flush};
use crate::linux::x11::composition::{
    Geometry, OverlayPlan, OverlayShape, OverlayWindowIds, OverlayWindowSpec, compositing_facts,
    create_native_child, create_overlay_window, cutout_in_child, destroy_overlay_window, extent,
    focus_inside, place_window, plan_overlay, raise_overlay_window, require_shape,
    select_forwarded_pointer_events, set_cutout_shape, set_mapped, set_overlay_shape,
    stack_siblings,
};
use crate::linux::{AuxWindow, AuxWindowKind, X11ClientStatePtr, check_compositor_present};

#[cfg(test)]
thread_local! {
    /// Compositing facts that replace the server's, so tests can drive the SHAPE fallback on a
    /// compositing server.
    pub(super) static FORCED_FACTS: Cell<Option<X11CompositingFacts>> = const { Cell::new(None) };
}

fn native_device(xcb: &XCBConnection) -> NativeDevice {
    let connection = as_raw_xcb_connection::AsRawXcbConnection::as_raw_xcb_connection(xcb);
    NativeDevice::X11 {
        connection: NonNull::new(connection as *mut c_void)
            .expect("an open X connection has a non-null xcb pointer"),
    }
}

#[cfg(test)]
thread_local! {
    /// The windows created on this thread, so tests can reach the state behind a platform
    /// window.
    static WINDOWS: RefCell<Vec<X11WindowStatePtr>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn track_window(window: &X11WindowStatePtr) {
    WINDOWS.with_borrow_mut(|windows| windows.push(window.clone()));
}

/// A native child window and what the framework last asked of it.
pub(super) struct NativeChild {
    xcb: Rc<XCBConnection>,
    client: X11ClientStatePtr,
    window: xproto::Window,
    owner: xproto::Window,
    generation: Rc<Cell<u64>>,
    /// The child's origin in window-content device pixels, shared with the client's input
    /// routing.
    origin: Rc<Cell<Point<i32>>>,
    state: RefCell<NativeChildState>,
}

struct NativeChildState {
    alive: bool,
    /// The X parent: the window, or another native child.
    x_parent: xproto::Window,
    bounds: Option<Bounds<DevicePixels>>,
    visible: bool,
    mapped: bool,
    forwarding: InputForwarding,
    recreated: Vec<Box<dyn FnMut(NativeComposition)>>,
    /// The cutout last applied to the child's shapes (PLT-001 R25).
    cutout: OcclusionTracker,
}

impl NativeChild {
    fn composition(&self) -> NativeComposition {
        NativeComposition {
            device: native_device(&self.xcb),
            layer: NativeLayer::X11 {
                window: self.window,
            },
            generation: self.generation.get(),
        }
    }

    fn ensure_alive(&self, state: &NativeChildState) -> Result<()> {
        ensure!(state.alive, "the native surface was destroyed");
        Ok(())
    }

    /// Maps the child when it is visible and has area, and unmaps it otherwise.
    fn sync_mapped(&self, state: &mut NativeChildState) -> Result<()> {
        let want = state.visible
            && state
                .bounds
                .is_some_and(|bounds| !Geometry::is_degenerate(bounds));
        if want != state.mapped {
            set_mapped(&*self.xcb, self.window, want)?;
            state.mapped = want;
            if want {
                // Its shapes were not kept up to date while it was hidden.
                state.cutout.invalidate();
            }
        }
        Ok(())
    }

    fn destroy(&self) {
        let mut state = self.state.borrow_mut();
        if !std::mem::replace(&mut state.alive, false) {
            return;
        }
        drop(state);
        self.client.unregister_aux_window(self.window);
        // Destroying the window releases its shapes.
        self.xcb.destroy_window(self.window).log_err();
    }

    /// Runs the callbacks registered with [`NativeAttachment::on_recreated`].
    fn fire_recreated(&self) {
        let mut callbacks = std::mem::take(&mut self.state.borrow_mut().recreated);
        let composition = self.composition();
        for callback in &mut callbacks {
            callback(composition);
        }
        let mut state = self.state.borrow_mut();
        callbacks.append(&mut state.recreated);
        state.recreated = callbacks;
    }

    /// Cuts `cutout` (window-content device pixels) out of the child's shapes, if it differs
    /// from what is applied (PLT-001 R23-R25).
    fn apply_cutout(&self, cutout: Vec<Bounds<DevicePixels>>) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if !state.alive || !state.mapped {
            return Ok(());
        }
        let Some(bounds) = state.bounds else {
            return Ok(());
        };
        let Some(applied) = state.cutout.update(cutout) else {
            return Ok(());
        };
        let local = cutout_in_child(applied, bounds);
        let result = set_cutout_shape(&*self.xcb, self.window, extent(bounds), &local);
        if result.is_err() {
            // Applying it failed: try again next frame.
            state.cutout.invalidate();
        }
        result
    }

    fn reparent(&self, x_parent: xproto::Window, parent_origin: Point<DevicePixels>) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if !state.alive || state.x_parent == x_parent {
            return Ok(());
        }
        let (x, y) = state.bounds.map_or((0, 0), |bounds| {
            let geometry = Geometry::relative(bounds, parent_origin);
            (geometry.x, geometry.y)
        });
        self.xcb.reparent_window(self.window, x_parent, x, y)?;
        state.x_parent = x_parent;
        Ok(())
    }
}

struct X11Attachment {
    child: Rc<NativeChild>,
}

impl NativeAttachment for X11Attachment {
    fn set_geometry(
        &self,
        bounds: Bounds<DevicePixels>,
        parent_origin: Point<DevicePixels>,
    ) -> Result<()> {
        let child = &self.child;
        let mut state = child.state.borrow_mut();
        child.ensure_alive(&state)?;
        // Queued, not flushed: the window flushes the whole frame's batch right before it
        // presents (PLT-001 R17).
        place_window(
            &*child.xcb,
            child.window,
            Geometry::relative(bounds, parent_origin),
        )?;
        state.bounds = Some(bounds);
        child
            .origin
            .set(Point::new(bounds.origin.x.0, bounds.origin.y.0));
        state.cutout.invalidate();
        child.sync_mapped(&mut state)
    }

    fn set_visible(&self, visible: bool) -> Result<()> {
        let child = &self.child;
        let mut state = child.state.borrow_mut();
        child.ensure_alive(&state)?;
        let previous = std::mem::replace(&mut state.visible, visible);
        let result = child.sync_mapped(&mut state);
        if result.is_err() {
            state.visible = previous;
        }
        result
    }

    fn layer(&self) -> NativeLayer {
        NativeLayer::X11 {
            window: self.child.window,
        }
    }

    fn generation(&self) -> u64 {
        self.child.generation.get()
    }

    fn on_recreated(&self, callback: Box<dyn FnMut(NativeComposition)>) {
        self.child.state.borrow_mut().recreated.push(callback);
    }

    fn set_input_forwarding(&self, forwarding: InputForwarding) {
        let child = &self.child;
        let mut state = child.state.borrow_mut();
        if !state.alive || state.forwarding == forwarding {
            return;
        }
        let forward = forwarding == InputForwarding::Framework;
        if select_forwarded_pointer_events(&*child.xcb, child.window, forward)
            .log_err()
            .is_none()
        {
            return;
        }
        state.forwarding = forwarding;
        drop(state);
        if forward {
            child.client.register_aux_window(
                child.window,
                AuxWindow {
                    owner: child.owner,
                    kind: AuxWindowKind::Native {
                        origin: child.origin.clone(),
                    },
                },
            );
        } else {
            child.client.unregister_aux_window(child.window);
        }
        xcb_flush(&child.xcb);
    }
}

/// The transparent override-redirect window the framework content above native children is
/// drawn into in composited mode (PLT-001 R22).
struct OverlayWindow {
    ids: OverlayWindowIds,
    /// Presents to `ids.window`; shares the window renderer's device and atlas.
    renderer: WgpuRenderer,
    geometry: Option<Geometry>,
    mapped: bool,
    shape: Option<OverlayShape>,
    /// The window shows a frame without pixels.
    cleared: bool,
}

/// The state of a window that opted into composition.
pub(super) struct X11Composition {
    /// The general window composition was enabled, not just the two-plane overlay.
    general: bool,
    /// Changes on every compositor-surface recreation; shared with the attachments.
    generation: Rc<Cell<u64>>,
    /// Composited mode or SHAPE fallback, decided the first time it is needed and cached.
    mode: Option<X11CompositionMode>,
    layout: Option<Rc<CompositionLayout>>,
    natives: HashMap<SurfaceId, Rc<NativeChild>>,
    /// The stacking last sent: the children of each X parent, bottom to top.
    stacked: Vec<(xproto::Window, Vec<xproto::Window>)>,
    overlay: Option<OverlayWindow>,
    /// The renderer of the last overlay window, kept for the next one.
    spare: Option<WgpuRenderer>,
    /// The overlay's input region in window-content logical pixels.
    region: Vec<Bounds<Pixels>>,
    /// Device recovery already changed the generation for the renderers it made stale.
    recovery_counted: bool,
}

impl X11Composition {
    fn new() -> Self {
        X11Composition {
            general: false,
            generation: Rc::new(Cell::new(0)),
            mode: None,
            layout: None,
            natives: HashMap::default(),
            stacked: Vec::new(),
            overlay: None,
            spare: None,
            region: Vec::new(),
            recovery_counted: false,
        }
    }

    /// The natives that are shown.
    fn shown_natives(&self) -> Vec<(SurfaceId, Rc<NativeChild>)> {
        self.natives
            .iter()
            .filter(|(_, child)| child.state.borrow().mapped)
            .map(|(id, child)| (*id, child.clone()))
            .collect()
    }

    /// A compositor surface was recreated: the generation changes and the attachments that must
    /// remount are returned for notification once no borrow is held.
    fn bump_generation(&mut self) -> Vec<Rc<NativeChild>> {
        self.generation.set(self.generation.get() + 1);
        self.natives.values().cloned().collect()
    }

    /// Releases the overlay window and its renderer, then the native children (PLT-001 R13).
    pub(super) fn release(&mut self, xcb: &XCBConnection, client: &X11ClientStatePtr) {
        self.destroy_overlay(xcb, client);
        self.spare = None;
        for (_, child) in self.natives.drain() {
            child.destroy();
        }
    }

    /// Destroys the overlay window (not merely unmaps it: some compositors raise a window
    /// together with only its mapped transients and would leave a remapped overlay below a
    /// floating parent), keeping its renderer as a spare for the next overlay window.
    fn destroy_overlay(&mut self, xcb: &XCBConnection, client: &X11ClientStatePtr) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        let OverlayWindow {
            ids, mut renderer, ..
        } = overlay;
        client.unregister_aux_window(ids.window);
        // The GPU surface goes before the window it presents to.
        renderer.unconfigure_surface();
        destroy_overlay_window(xcb, ids);
        if renderer.is_reusable() {
            self.spare = Some(renderer);
        }
    }
}

/// Rendering context of one overlay frame.
struct FrameContext<'a> {
    xcb: &'a Rc<XCBConnection>,
    client: &'a X11ClientStatePtr,
    gpu_context: &'a GpuContext,
    owner: xproto::Window,
    root: xproto::Window,
    screen_index: usize,
    visual_id: u32,
    visual_depth: u8,
    scale_factor: f32,
    atoms: &'a super::XcbAtoms,
    window_size: Size<DevicePixels>,
    atlas: Arc<vespertine_wgpu::WgpuAtlas>,
}

use std::sync::Arc;

/// What a platform draw entry hands to composition.
pub(super) enum Frame<'a> {
    /// The two-plane entry: the whole scene and where the overlay starts.
    Layered {
        scene: &'a Scene,
        overlay_start: usize,
        capture: bool,
    },
    /// One scene per framework surface in tree order.
    Composed(&'a ComposedScene),
}

/// Every layer of `composed` in tree order as one scene: the window's own content.
fn merge_layers(composed: &ComposedScene) -> Scene {
    let mut merged = Scene::default();
    for layer in &composed.layers {
        merged.replay(0..layer.scene.len(), &layer.scene);
    }
    merged.finish();
    merged
}

/// The scene of the overlay window: `ids` of `composed` as one scene.
fn overlay_scene<'a>(
    composed: &'a ComposedScene,
    ids: &[SurfaceId],
    merged: &'a mut Option<Scene>,
) -> &'a Scene {
    if let [only] = ids
        && let Some(layer) = composed.layer(*only)
    {
        return &layer.scene;
    }
    let mut scene = Scene::default();
    for id in ids {
        if let Some(layer) = composed.layer(*id) {
            scene.replay(0..layer.scene.len(), &layer.scene);
        }
    }
    scene.finish();
    merged.insert(scene)
}

/// What the server offers for compositing the window's screen.
fn gather_facts(
    xcb: &XCBConnection,
    root: xproto::Window,
    screen_index: usize,
    visual_depth: u8,
) -> X11CompositingFacts {
    #[cfg(test)]
    if let Some(forced) = FORCED_FACTS.get() {
        return forced;
    }
    compositing_facts(xcb, visual_depth == 32, || {
        check_compositor_present(xcb, root, screen_index)
    })
}

impl X11Composition {
    fn mode(
        &mut self,
        xcb: &XCBConnection,
        root: xproto::Window,
        screen_index: usize,
        visual_depth: u8,
    ) -> X11CompositionMode {
        if let Some(mode) = self.mode {
            return mode;
        }
        let facts = gather_facts(xcb, root, screen_index, visual_depth);
        let mode = decide_x11_composition_mode(facts);
        log::info!("x11: scene layering uses {mode:?} ({facts:?})");
        self.mode = Some(mode);
        mode
    }

    /// Makes sure an overlay window with a usable renderer exists. A renderer that reports a
    /// lost device is discarded and recreated from the current device (PLT-001 R18).
    fn ensure_overlay(
        &mut self,
        context: &FrameContext<'_>,
    ) -> Result<(Vec<Rc<NativeChild>>, &mut OverlayWindow)> {
        let mut recreated = Vec::new();
        let mut note_recreation = |this: &mut Self| {
            if !std::mem::take(&mut this.recovery_counted) {
                recreated.extend(this.bump_generation());
            }
        };
        if self
            .overlay
            .as_ref()
            .is_some_and(|overlay| !overlay.renderer.is_reusable())
        {
            self.destroy_overlay(context.xcb, context.client);
            note_recreation(self);
        }
        if self
            .spare
            .as_ref()
            .is_some_and(|renderer| !renderer.is_reusable())
        {
            self.spare = None;
            note_recreation(self);
        }
        if self.overlay.is_none() {
            let overlay = self.create_overlay(context)?;
            self.overlay = Some(overlay);
        }
        let overlay = self
            .overlay
            .as_mut()
            .expect("the overlay window exists after it was created");
        Ok((recreated, overlay))
    }

    fn create_overlay(&mut self, context: &FrameContext<'_>) -> Result<OverlayWindow> {
        let geometry = overlay_geometry(context)?;
        let ids = create_overlay_window(
            &**context.xcb,
            &OverlayWindowSpec {
                root: context.root,
                visual: context.visual_id,
                depth: context.visual_depth,
                owner: context.owner,
                geometry,
                atoms: context.atoms,
            },
        )?;
        let raw_window = RawWindow {
            connection: as_raw_xcb_connection::AsRawXcbConnection::as_raw_xcb_connection(
                &**context.xcb,
            ) as *mut _,
            screen_id: context.screen_index,
            window_id: ids.window,
            visual_id: context.visual_id,
        };
        let config = WgpuSurfaceConfig {
            size: context.window_size,
            transparent: true,
            // Not blocking: the window presents with vsync right after.
            preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
        };
        let renderer = match self.spare.take() {
            Some(mut spare) => {
                let instance = context
                    .gpu_context
                    .borrow()
                    .as_ref()
                    .map(|gpu| gpu.instance.clone());
                instance
                    .context("the GPU device is gone")
                    .and_then(|instance| spare.replace_surface(&raw_window, config, &instance))
                    .map(|()| spare)
            }
            None => WgpuRenderer::new_sharing_atlas(
                context.gpu_context.clone(),
                &raw_window,
                config,
                None,
                context.atlas.clone(),
            ),
        };
        let renderer = match renderer {
            Ok(renderer) => renderer,
            Err(error) => {
                destroy_overlay_window(&**context.xcb, ids);
                return Err(error.context("creating the overlay renderer failed"));
            }
        };
        context.client.register_aux_window(
            ids.window,
            AuxWindow {
                owner: context.owner,
                kind: AuxWindowKind::Overlay,
            },
        );
        Ok(OverlayWindow {
            ids,
            renderer,
            geometry: Some(geometry),
            mapped: false,
            shape: None,
            cleared: false,
        })
    }
}

/// The overlay window's geometry: the window's device-pixel size at the window content's
/// root-relative origin.
fn overlay_geometry(context: &FrameContext<'_>) -> Result<Geometry> {
    let origin = get_reply(
        || "X11 TranslateCoordinates for the overlay window failed.",
        context
            .xcb
            .translate_coordinates(context.owner, context.root, 0, 0),
    )?;
    let (width, height) = extent(Bounds {
        origin: Point::default(),
        size: context.window_size,
    });
    Ok(Geometry {
        x: origin.dst_x,
        y: origin.dst_y,
        width: width.max(1),
        height: height.max(1),
    })
}

impl X11WindowStatePtr {
    /// Enables composition: the window keeps drawing a single plane (and this fails) without the
    /// SHAPE extension (PLT-001 R1, R21).
    pub(super) fn enable_composition(&self, general: bool) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if state.composition.is_none() {
            require_shape(&*self.xcb)?;
            state.composition = Some(X11Composition::new());
        }
        if general && let Some(composition) = state.composition.as_mut() {
            composition.general = true;
        }
        Ok(())
    }

    pub(super) fn native_composition(&self) -> Option<NativeComposition> {
        let state = self.state.try_borrow().ok()?;
        let composition = state.composition.as_ref()?;
        Some(NativeComposition {
            device: native_device(&self.xcb),
            layer: NativeLayer::X11 {
                window: self.x_window,
            },
            generation: composition.generation.get(),
        })
    }

    pub(super) fn set_overlay_region(&self, region: &[Bounds<Pixels>]) {
        if let Some(composition) = self.state.borrow_mut().composition.as_mut()
            && composition.region != region
        {
            composition.region = region.to_vec();
        }
    }

    pub(super) fn create_native_attachment(
        &self,
        surface: SurfaceId,
        kind: SurfaceKind,
    ) -> Result<Rc<dyn NativeAttachment>> {
        ensure!(
            kind.is_attachment(),
            "a {kind:?} surface has no native window"
        );
        let mut guard = self.state.borrow_mut();
        let state = &mut *guard;
        let composition = state
            .composition
            .as_mut()
            .filter(|composition| composition.general)
            .context("window composition is not enabled")?;
        let screen = &self.xcb.setup().roots[state.x_screen_index];
        let window = create_native_child(&*self.xcb, screen, self.x_window)?;
        let child = Rc::new(NativeChild {
            xcb: self.xcb.clone(),
            client: state.client.clone(),
            window,
            owner: self.x_window,
            generation: composition.generation.clone(),
            origin: Rc::new(Cell::new(Point::default())),
            state: RefCell::new(NativeChildState {
                alive: true,
                x_parent: self.x_window,
                bounds: None,
                visible: false,
                mapped: false,
                forwarding: InputForwarding::Native,
                recreated: Vec::new(),
                cutout: OcclusionTracker::default(),
            }),
        });
        composition.natives.insert(surface, child.clone());
        Ok(Rc::new(X11Attachment { child }))
    }

    /// Makes the native child windows follow `layout`: surfaces that are not listed are
    /// destroyed, children are reparented under their parent attachment and stacked bottom to
    /// top (PLT-001 R3, R4, R21). Nothing changes when the layout cannot be realized.
    pub(super) fn sync_composition_order(&self, layout: &CompositionLayout) -> Result<()> {
        let mut guard = self.state.borrow_mut();
        let composition = guard
            .composition
            .as_mut()
            .filter(|composition| composition.general)
            .context("window composition is not enabled")?;
        let base_position = layout.position(layout.base).unwrap_or(0);
        for (position, entry) in layout.entries.iter().enumerate() {
            if entry.kind.is_attachment() && entry.parent.is_none() && position < base_position {
                bail!(
                    "X11 draws the window's own content into the window, so the native surface {:?} cannot stack below it",
                    entry.id
                );
            }
        }
        composition.natives.retain(|id, child| {
            let listed = layout.entry(*id).is_some();
            if !listed {
                child.destroy();
            }
            listed
        });
        let mut groups: Vec<(xproto::Window, Vec<xproto::Window>)> = Vec::new();
        for entry in &layout.entries {
            let Some(child) = composition.natives.get(&entry.id) else {
                continue;
            };
            let parent = entry
                .parent
                .and_then(|parent| composition.natives.get(&parent));
            let (x_parent, parent_origin) = match parent {
                Some(parent) => (
                    parent.window,
                    parent
                        .state
                        .borrow()
                        .bounds
                        .map(|bounds| bounds.origin)
                        .unwrap_or_default(),
                ),
                None => (self.x_window, Point::default()),
            };
            child.reparent(x_parent, parent_origin)?;
            match groups.iter_mut().find(|(parent, _)| *parent == x_parent) {
                Some((_, windows)) => windows.push(child.window),
                None => groups.push((x_parent, vec![child.window])),
            }
        }
        if composition.stacked != groups {
            for (_, windows) in &groups {
                stack_siblings(&*self.xcb, windows)?;
            }
            composition.stacked = groups;
        }
        for child in composition.natives.values() {
            child.state.borrow_mut().cutout.invalidate();
        }
        composition.layout = Some(Rc::new(layout.clone()));
        drop(guard);
        xcb_flush(&self.xcb);
        Ok(())
    }

    /// Whether a native child (or a window inside one) holds the keyboard focus.
    pub(super) fn native_focus_active(&self) -> bool {
        if self
            .state
            .try_borrow()
            .map_or(true, |state| state.composition.is_none())
        {
            return false;
        }
        get_reply(|| "X11 GetInputFocus failed.", self.xcb.get_input_focus())
            .log_err()
            .is_some_and(|reply| focus_inside(&*self.xcb, self.x_window, reply.focus))
    }

    /// Moves the keyboard focus from a native child to the framework window.
    pub(super) fn focus_framework_window(&self) {
        self.xcb
            .set_input_focus(
                xproto::InputFocus::PARENT,
                self.x_window,
                x11rb::CURRENT_TIME,
            )
            .log_err();
        xcb_flush(&self.xcb);
    }

    /// Hands a finished frame to the X server: the overlay window and the shapes of the native
    /// children first, the window's own content last (PLT-001 R5-R7, R17, R21-R25).
    pub(super) fn present_composition(&self, frame: Frame<'_>) -> Result<()> {
        // The window's own content is every framework surface in composition order.
        let merged;
        let parent_scene: &Scene = match &frame {
            Frame::Layered { scene, .. } => scene,
            Frame::Composed(composed) => {
                merged = merge_layers(composed);
                &merged
            }
        };
        match self.prepare_planes(&frame) {
            Ok(PlanesOutcome::Ready(recreated)) => {
                notify_recreated(recreated);
            }
            Ok(PlanesOutcome::Unbalanced) => return Ok(()),
            // The window's own content holds the overlay content too: it still draws.
            Err(error) => log::error!("preparing the overlay planes failed: {error:#}"),
        }
        self.draw_scene_and_note_recovery(parent_scene);
        Ok(())
    }

    /// The overlay window, native shapes, flush and overlay frame of one presentation.
    fn prepare_planes(&self, frame: &Frame<'_>) -> Result<PlanesOutcome> {
        let mut guard = self.state.borrow_mut();
        let state = &mut *guard;
        let Some(composition) = state.composition.as_mut() else {
            return Ok(PlanesOutcome::Ready(Vec::new()));
        };
        let Some(renderer) = state.renderer.as_mut() else {
            return Ok(PlanesOutcome::Ready(Vec::new()));
        };
        let layout = composition.layout.clone();
        let shown = composition.shown_natives();
        let unavailable = renderer.device_lost();
        let (Some(layout), false, false) = (layout, shown.is_empty(), unavailable) else {
            // Nothing to be above: no overlay window, and the window draws everything itself.
            composition.destroy_overlay(&self.xcb, &state.client);
            return Ok(PlanesOutcome::Ready(Vec::new()));
        };

        // The per-surface scenes the cutouts and the overlay window are made from.
        let split;
        let composed: &ComposedScene = match frame {
            Frame::Composed(composed) => composed,
            Frame::Layered {
                scene,
                overlay_start,
                capture,
            } => match scene.split_at_overlay(*overlay_start) {
                Ok((base, overlay)) => {
                    split = ComposedScene {
                        layers: vec![
                            vespertine::ComposedLayer {
                                surface: layout.base,
                                scene: base,
                                hitboxes: Vec::new(),
                            },
                            vespertine::ComposedLayer {
                                surface: layout.overlay,
                                scene: overlay,
                                hitboxes: composition.region.clone(),
                            },
                        ],
                        capture: *capture,
                    };
                    &split
                }
                Err(error) => {
                    log::error!(
                        "the frame is not drawn, its clip scopes cannot be balanced: {error}"
                    );
                    return Ok(PlanesOutcome::Unbalanced);
                }
            },
        };

        let screen_index = state.x_screen_index;
        let root = state.x_root_window;
        let mode = composition.mode(&self.xcb, root, screen_index, state.visual_depth);

        // Each native child is cut by the layers above it in tree order only.
        for (id, child) in &shown {
            child
                .apply_cutout(native_cutout(&layout, composed, *id, mode))
                .log_err();
        }

        let shown_ids: Vec<SurfaceId> = shown.iter().map(|(id, _)| *id).collect();
        let plan = if mode == X11CompositionMode::Composited {
            plan_overlay(&layout, &shown_ids)
        } else {
            OverlayPlan::default()
        };
        let mut merged = None;
        let overlay_content = plan
            .has_window()
            .then(|| overlay_scene(composed, &plan.window_layers, &mut merged));
        let context = FrameContext {
            xcb: &self.xcb,
            client: &state.client,
            gpu_context: &state.gpu_context,
            owner: self.x_window,
            root,
            screen_index,
            visual_id: state.visual_id,
            visual_depth: state.visual_depth,
            scale_factor: state.scale_factor,
            atoms: &state.atoms,
            window_size: renderer.viewport_size(),
            atlas: renderer.sprite_atlas().clone(),
        };
        let parent_hidden =
            state.hidden || state.visibility == vespertine::WindowVisibility::Hidden;
        let needed = !parent_hidden
            && overlay_content.is_some()
            && overlay_window_needed(
                overlay_content.is_some_and(|scene| !scene.is_empty()),
                !composition.region.is_empty(),
            );
        let region = composition.region.clone();
        let mut recreated = Vec::new();
        let mut overlay_frame = None;
        if needed {
            let (overlay_recreated, overlay) = composition.ensure_overlay(&context)?;
            recreated = overlay_recreated;
            update_overlay(overlay, &context, &region, &plan, &shown)?;
            overlay_frame = overlay_content;
        } else {
            composition.destroy_overlay(&self.xcb, &state.client);
        }
        // One batch: the children's geometry queued by the presentation callbacks, their
        // shapes, and the overlay window's geometry, shape and restack.
        xcb_flush(&self.xcb);
        if let (Some(overlay), Some(scene)) = (composition.overlay.as_mut(), overlay_frame) {
            draw_overlay(overlay, scene);
        }
        Ok(PlanesOutcome::Ready(recreated))
    }
}

/// Brings the overlay window's geometry, shape and stacking up to date: geometry and shape are
/// only rewritten when they changed, the window is mapped and restacked above its owner on every
/// frame (PLT-001 R22).
fn update_overlay(
    overlay: &mut OverlayWindow,
    context: &FrameContext<'_>,
    region: &[Bounds<Pixels>],
    plan: &OverlayPlan,
    shown: &[(SurfaceId, Rc<NativeChild>)],
) -> Result<()> {
    let geometry = overlay_geometry(context)?;
    if overlay.geometry != Some(geometry) || !overlay.mapped {
        place_window(&**context.xcb, overlay.ids.window, geometry)?;
        overlay.geometry = Some(geometry);
        overlay.renderer.update_drawable_size(Size {
            width: DevicePixels(i32::from(geometry.width)),
            height: DevicePixels(i32::from(geometry.height)),
        });
        overlay.cleared = false;
    }
    let holes: Vec<Bounds<DevicePixels>> = shown
        .iter()
        .filter(|(id, _)| plan.hosted_above.contains(id))
        .filter_map(|(_, child)| child.state.borrow().bounds)
        .collect();
    let shape = OverlayShape::new(
        (geometry.width, geometry.height),
        region,
        context.scale_factor,
        &holes,
    );
    if overlay.shape.as_ref() != Some(&shape) {
        set_overlay_shape(&**context.xcb, overlay.ids.window, &shape)?;
        overlay.shape = Some(shape);
    }
    raise_overlay_window(&**context.xcb, overlay.ids.window, context.owner)?;
    overlay.mapped = true;
    Ok(())
}

/// Draws the overlay window's scene. A frame without primitives is only drawn once: the window
/// keeps showing it.
fn draw_overlay(overlay: &mut OverlayWindow, scene: &Scene) {
    if scene.is_empty() && overlay.cleared {
        return;
    }
    if overlay.renderer.draw(scene) {
        overlay.cleared = scene.is_empty();
    }
}

enum PlanesOutcome {
    /// The overlay is up to date; attachments whose compositor surface was recreated are to be
    /// notified.
    Ready(Vec<Rc<NativeChild>>),
    /// The scopes of the frame cannot be balanced: it is not drawn.
    Unbalanced,
}

/// Runs the recreation callbacks of `children`; no window state may be borrowed.
fn notify_recreated(children: Vec<Rc<NativeChild>>) {
    for child in children {
        child.fire_recreated();
    }
}

impl X11WindowStatePtr {
    /// Draws the window's own scene and, when that recovered the GPU device, signals the
    /// recreation of the compositor surface to the attachments (PLT-001 R18).
    pub(super) fn draw_scene_and_note_recovery(&self, scene: &Scene) {
        if self.draw_scene(scene) {
            self.note_device_recovered();
        }
    }

    fn note_device_recovered(&self) {
        let recreated = {
            let mut state = self.state.borrow_mut();
            let Some(composition) = state.composition.as_mut() else {
                return;
            };
            // The overlay and spare renderers were made on the lost device; the next composed
            // frame discards them without changing the generation a second time.
            composition.recovery_counted = true;
            composition.bump_generation()
        };
        notify_recreated(recreated);
    }

    /// The overlay window must not outlive the visibility of its owner: an override-redirect
    /// window would stay on screen over a minimized or unmapped window.
    pub(super) fn release_overlay_while_hidden(&self, state: &mut X11WindowState) {
        if !(state.hidden || state.visibility == vespertine::WindowVisibility::Hidden) {
            return;
        }
        let client = state.client.clone();
        if let Some(composition) = state.composition.as_mut() {
            composition.destroy_overlay(&self.xcb, &client);
        }
    }
}

impl X11Window {
    pub(super) fn composition_enable(&self, general: bool) -> Result<()> {
        self.0.enable_composition(general)
    }

    pub(super) fn composition_draw_layered(
        &self,
        scene: &Scene,
        overlay_start: usize,
        capture: bool,
    ) {
        let has_composition = self.0.state.borrow().composition.is_some();
        if !has_composition {
            self.0.draw_scene_and_note_recovery(scene);
            return;
        }
        self.0
            .present_composition(Frame::Layered {
                scene,
                overlay_start,
                capture,
            })
            .log_err();
    }

    pub(super) fn composition_draw_composed(&self, composed: &ComposedScene) -> Result<()> {
        ensure!(
            self.0.state.borrow().composition.is_some(),
            "window composition is not enabled"
        );
        self.0.present_composition(Frame::Composed(composed))
    }
}

#[cfg(test)]
mod tests;
