//! Scene layering around native child content on Wayland (PLT-001 R19, R20).
//!
//! Every composition surface other than the window's own is a subsurface of it (or of another
//! composition surface):
//!
//! * Framework overlay surfaces are *synchronized* subsurfaces drawn by renderers that share the
//!   window renderer's GPU device and sprite atlas. Each overlay is drawn and committed first;
//!   the cached commits are released by the window surface commit that presents the base frame,
//!   so base and overlay appear atomically.
//! * Native surfaces are *desynchronized* subsurfaces that their producers present to
//!   independently. Their position, size and visibility are written from presentation callbacks
//!   and take effect with the window surface commit. Hiding a native surface destroys its
//!   `wl_subsurface` role object, which unmaps it; showing it creates the role object again.
//! * Stacking follows the flattened composition layout through `place_above`/`place_below`.
//! * Overlay input regions are the union of the overlay's hitboxes (empty without any), and
//!   pointer events entering an overlay surface resolve to the owning window.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ffi::c_void,
    ptr::NonNull,
    rc::{Rc, Weak},
    sync::Arc,
};

use anyhow::{Result, bail};
use raw_window_handle as rwh;
use wayland_backend::client::ObjectId;
use wayland_client::{
    Proxy,
    protocol::{wl_subsurface::WlSubsurface, wl_surface::WlSurface},
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

use vespertine::{
    Bounds, CompositionLayout, DevicePixels, InputForwarding, NativeAttachment, NativeComposition,
    NativeDevice, NativeLayer, Pixels, Point, Scene, Size, SurfaceId, SurfaceKind, px,
};
use vespertine_wgpu::{
    BackdropPlane, CompositorGpuHint, GpuContext, WgpuAtlas, WgpuRenderer, WgpuSurfaceConfig, wgpu,
};

use crate::linux::{
    Globals, SubsurfaceInput, WaylandClientStatePtr, wayland::window::WaylandWindowStatePtr,
};

/// A wgpu window handle for a composition surface.
#[derive(Debug, Clone, Copy)]
struct RawSurface {
    surface: *mut c_void,
    display: *mut c_void,
}

// Safety: the pointers name Wayland objects that outlive the renderer (renderers are released
// before the surfaces they present to), and wgpu only needs them to create its surface.
unsafe impl Send for RawSurface {}
unsafe impl Sync for RawSurface {}

impl rwh::HasWindowHandle for RawSurface {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        let surface = NonNull::new(self.surface).ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::WaylandWindowHandle::new(surface);
        Ok(unsafe { rwh::WindowHandle::borrow_raw(handle.into()) })
    }
}

impl rwh::HasDisplayHandle for RawSurface {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        let display = NonNull::new(self.display).ok_or(rwh::HandleError::Unavailable)?;
        let handle = rwh::WaylandDisplayHandle::new(display);
        Ok(unsafe { rwh::DisplayHandle::borrow_raw(handle.into()) })
    }
}

fn raw_surface(surface: &WlSurface) -> Option<RawSurface> {
    Some(RawSurface {
        surface: surface.id().as_ptr().cast(),
        display: surface.backend().upgrade()?.display_ptr().cast(),
    })
}

/// State shared between the window's composition and its native attachments.
struct Env {
    /// The window's scale factor, to convert device-pixel geometry to surface coordinates.
    scale: Cell<f32>,
    /// The compositor-surface generation (PLT-001 R18).
    generation: Cell<u64>,
    /// A native surface was mapped or recreated: the stacking order must be applied again.
    restack: Cell<bool>,
    /// A compositor surface was recreated since the embedders were told.
    recreated: Cell<bool>,
}

impl Env {
    fn bump_generation(&self) {
        self.generation.set(self.generation.get() + 1);
        self.recreated.set(true);
    }
}

/// A framework overlay plane: a synchronized subsurface covering the window.
struct OverlayRole {
    /// Dropped first: a renderer releases its GPU surface before the surface it presents to.
    renderer: Option<WgpuRenderer>,
    id: Option<SurfaceId>,
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: Option<WpViewport>,
    parent: WlSurface,
    /// A frame is attached; otherwise the plane presents no pixels.
    mapped: bool,
    /// The hitbox region last written to the surface, in logical pixels.
    region: Option<Vec<Bounds<Pixels>>>,
    /// State was written that only a commit of the surface latches.
    needs_commit: bool,
}

/// The Wayland side of a window with composition enabled.
pub(crate) struct WaylandComposition {
    /// Overlay planes; the first is the window's overlay surface.
    overlays: Vec<OverlayRole>,
    natives: HashMap<SurfaceId, Weak<WaylandNative>>,
    layout: Option<CompositionLayout>,
    env: Rc<Env>,
    globals: Globals,
    client: WaylandClientStatePtr,
    window: WaylandWindowStatePtr,
    window_surface: WlSurface,
    gpu_context: GpuContext,
    compositor_gpu: Option<CompositorGpuHint>,
    logical_size: Size<Pixels>,
    device_size: Size<DevicePixels>,
    /// The layout stacking was last applied for.
    stacked: Option<CompositionLayout>,
}

impl WaylandComposition {
    /// Enables the two-plane overlay. Fails when the compositor offers no subcompositor.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        window: WaylandWindowStatePtr,
        window_surface: WlSurface,
        globals: Globals,
        client: WaylandClientStatePtr,
        gpu_context: GpuContext,
        compositor_gpu: Option<CompositorGpuHint>,
        logical_size: Size<Pixels>,
        device_size: Size<DevicePixels>,
        scale: f32,
    ) -> Result<Self> {
        if globals.subcompositor.is_none() {
            bail!(
                "the compositor does not provide wl_subcompositor, so surfaces cannot be layered"
            );
        }
        let mut composition = WaylandComposition {
            overlays: Vec::new(),
            natives: HashMap::new(),
            layout: None,
            env: Rc::new(Env {
                scale: Cell::new(scale),
                generation: Cell::new(1),
                restack: Cell::new(false),
                recreated: Cell::new(false),
            }),
            globals,
            client,
            window,
            window_surface,
            gpu_context,
            compositor_gpu,
            logical_size,
            device_size,
            stacked: None,
        };
        let parent = composition.window_surface.clone();
        let overlay = composition.create_overlay(None, parent)?;
        composition.overlays.push(overlay);
        Ok(composition)
    }

    fn create_overlay(&self, id: Option<SurfaceId>, parent: WlSurface) -> Result<OverlayRole> {
        let Some(subcompositor) = self.globals.subcompositor.as_ref() else {
            bail!("the compositor does not provide wl_subcompositor");
        };
        let surface = self.globals.compositor.create_surface(&self.globals.qh, ());
        let subsurface = subcompositor.get_subsurface(&surface, &parent, &self.globals.qh, ());
        // Synchronized: the overlay's frames are released by the window surface commit.
        subsurface.set_sync();
        subsurface.set_position(0, 0);
        // Pointer events pass to the surfaces below until hitboxes say otherwise.
        let empty = self.globals.compositor.create_region(&self.globals.qh, ());
        surface.set_input_region(Some(&empty));
        empty.destroy();
        let viewport = self
            .globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, &self.globals.qh, ()));
        if let Some(viewport) = &viewport {
            viewport.set_destination(
                f32::from(self.logical_size.width) as i32,
                f32::from(self.logical_size.height) as i32,
            );
        } else {
            surface.set_buffer_scale(self.env.scale.get().round().max(1.0) as i32);
        }
        self.client.register_subsurface(
            surface.id(),
            SubsurfaceInput {
                window: self.window.clone(),
                framework_pointer: Rc::new(Cell::new(true)),
                origin: Rc::new(Cell::new(Point::default())),
                native: false,
            },
        );
        Ok(OverlayRole {
            renderer: None,
            id,
            surface,
            subsurface,
            viewport,
            parent,
            mapped: false,
            region: Some(Vec::new()),
            needs_commit: true,
        })
    }

    fn destroy_overlay(&self, mut overlay: OverlayRole) {
        overlay.renderer.take();
        self.client.unregister_subsurface(&overlay.surface.id());
        if let Some(viewport) = &overlay.viewport {
            viewport.destroy();
        }
        overlay.subsurface.destroy();
        overlay.surface.destroy();
    }

    /// Releases overlay renderers, overlay surfaces and native roles in dependency order
    /// (PLT-001 R13). The window's own surface is destroyed afterwards by the caller.
    pub(crate) fn teardown(&mut self) {
        for overlay in std::mem::take(&mut self.overlays) {
            self.destroy_overlay(overlay);
        }
        for (_, native) in std::mem::take(&mut self.natives) {
            if let Some(native) = native.upgrade() {
                native.unmap();
            }
        }
    }

    /// The base surface of the last synchronized layout.
    pub(crate) fn base_surface(&self) -> Option<SurfaceId> {
        self.layout.as_ref().map(|layout| layout.base)
    }

    pub(crate) fn native_composition(&self) -> Option<NativeComposition> {
        let display = NonNull::new(raw_surface(&self.window_surface)?.display)?;
        let surface = NonNull::new(self.window_surface.id().as_ptr().cast::<c_void>())?;
        Some(NativeComposition {
            device: NativeDevice::Wayland { display },
            layer: NativeLayer::Wayland { surface },
            generation: self.env.generation.get(),
        })
    }

    /// The window resized or changed scale: every plane follows before the next presentation
    /// (PLT-001 R17).
    pub(crate) fn resize(
        &mut self,
        logical_size: Size<Pixels>,
        device_size: Size<DevicePixels>,
        scale: f32,
    ) {
        self.logical_size = logical_size;
        self.device_size = device_size;
        self.env.scale.set(scale);
        for overlay in &mut self.overlays {
            if let Some(renderer) = &mut overlay.renderer {
                renderer.update_drawable_size(device_size);
            }
            if let Some(viewport) = &overlay.viewport {
                viewport.set_destination(
                    f32::from(logical_size.width) as i32,
                    f32::from(logical_size.height) as i32,
                );
            } else {
                overlay
                    .surface
                    .set_buffer_scale(scale.round().max(1.0) as i32);
            }
        }
    }

    /// Creates the platform side of a native or external-GPU surface. It starts hidden.
    pub(crate) fn create_native(
        &mut self,
        id: SurfaceId,
        kind: SurfaceKind,
    ) -> Result<Rc<WaylandNative>> {
        if !kind.is_attachment() {
            bail!("only native and external-GPU surfaces have native attachments");
        }
        let surface = self.globals.compositor.create_surface(&self.globals.qh, ());
        let viewport = self
            .globals
            .viewporter
            .as_ref()
            .map(|viewporter| viewporter.get_viewport(&surface, &self.globals.qh, ()));
        let framework_pointer = Rc::new(Cell::new(false));
        let origin = Rc::new(Cell::new(Point::default()));
        self.client.register_subsurface(
            surface.id(),
            SubsurfaceInput {
                window: self.window.clone(),
                framework_pointer: framework_pointer.clone(),
                origin: origin.clone(),
                native: true,
            },
        );
        let native = Rc::new(WaylandNative {
            env: self.env.clone(),
            globals: self.globals.clone(),
            client: self.client.clone(),
            framework_pointer,
            origin,
            surface,
            viewport,
            state: RefCell::new(NativeState {
                subsurface: None,
                parent: self.window_surface.clone(),
                position: Point::default(),
                destination: None,
                visible: false,
                unmapped_once: false,
                generation: self.env.generation.get(),
                callbacks: Vec::new(),
            }),
        });
        self.natives.insert(id, Rc::downgrade(&native));
        Ok(native)
    }

    fn surface_of(&self, id: SurfaceId) -> Option<WlSurface> {
        let layout = self.layout.as_ref()?;
        if id == layout.base {
            return Some(self.window_surface.clone());
        }
        if let Some(overlay) = self.overlays.iter().find(|overlay| overlay.id == Some(id)) {
            return Some(overlay.surface.clone());
        }
        self.natives
            .get(&id)
            .and_then(Weak::upgrade)
            .map(|native| native.surface.clone())
    }

    /// Makes the surfaces follow `layout`: creates overlay planes for new framework entries,
    /// destroys surfaces that are not listed any more and stacks everything bottom to top.
    pub(crate) fn sync_order(&mut self, layout: &CompositionLayout) -> Result<()> {
        if self.globals.subcompositor.is_none() {
            bail!("the compositor does not provide wl_subcompositor");
        }
        // The window's overlay surface takes the identity the tree gave it.
        if let Some(first) = self.overlays.first_mut()
            && first.id.is_none()
        {
            first.id = Some(layout.overlay);
        }
        self.layout = Some(layout.clone());

        let listed =
            |id: SurfaceId| layout.entries.iter().any(|entry| entry.id == id) || id == layout.base;
        // Surfaces that left the tree.
        let mut index = 0;
        while index < self.overlays.len() {
            let keep = self.overlays[index].id.is_none_or(&listed);
            if keep {
                index += 1;
            } else {
                let overlay = self.overlays.remove(index);
                self.destroy_overlay(overlay);
            }
        }
        self.natives.retain(|id, native| {
            if listed(*id) {
                return true;
            }
            if let Some(native) = native.upgrade() {
                native.unmap();
            }
            false
        });

        // New framework surfaces and changed parents.
        for entry in &layout.entries {
            if entry.id == layout.base || entry.kind.is_attachment() {
                continue;
            }
            let parent = match entry.parent {
                None => self.window_surface.clone(),
                Some(parent) if parent == layout.base => self.window_surface.clone(),
                Some(parent) => self
                    .surface_of(parent)
                    .unwrap_or_else(|| self.window_surface.clone()),
            };
            match self
                .overlays
                .iter()
                .position(|overlay| overlay.id == Some(entry.id))
            {
                None => {
                    let overlay = self.create_overlay(Some(entry.id), parent)?;
                    self.overlays.push(overlay);
                }
                Some(position) if self.overlays[position].parent != parent => {
                    let id = Some(entry.id);
                    let old = self.overlays.remove(position);
                    self.destroy_overlay(old);
                    let overlay = self.create_overlay(id, parent)?;
                    self.overlays.insert(position, overlay);
                    self.bump_generation();
                }
                Some(_) => {}
            }
        }
        for entry in &layout.entries {
            if !entry.kind.is_attachment() {
                continue;
            }
            let parent = match entry.parent {
                None => self.window_surface.clone(),
                Some(parent) if parent == layout.base => self.window_surface.clone(),
                Some(parent) => self
                    .surface_of(parent)
                    .unwrap_or_else(|| self.window_surface.clone()),
            };
            if let Some(native) = self.natives.get(&entry.id).and_then(Weak::upgrade) {
                // Recreating the role object bumps the generation by itself.
                native.set_parent(&parent);
            }
        }
        self.apply_stacking();
        Ok(())
    }

    /// Applies the layout's stacking order to the subsurfaces that exist. Requests are
    /// double-buffered and take effect with the parent's next commit.
    fn apply_stacking(&mut self) {
        let Some(layout) = self.layout.clone() else {
            return;
        };
        self.env.restack.set(false);
        // Siblings per parent surface, in stacking order, hidden natives left out. The parent
        // itself anchors the sequence.
        let mut groups: Vec<(WlSurface, Vec<(WlSurface, Option<WlSubsurface>)>)> = Vec::new();
        let window = self.window_surface.clone();
        for entry in &layout.entries {
            let node = if entry.id == layout.base {
                Some((window.clone(), None))
            } else if let Some(overlay) = self.overlays.iter().find(|o| o.id == Some(entry.id)) {
                Some((overlay.surface.clone(), Some(overlay.subsurface.clone())))
            } else {
                self.natives
                    .get(&entry.id)
                    .and_then(Weak::upgrade)
                    .and_then(|native| {
                        let subsurface = native.state.borrow().subsurface.clone()?;
                        Some((native.surface.clone(), Some(subsurface)))
                    })
            };
            let Some(node) = node else { continue };
            let parent = match entry.parent {
                None => window.clone(),
                Some(parent) if parent == layout.base => window.clone(),
                Some(parent) => self.surface_of(parent).unwrap_or_else(|| window.clone()),
            };
            let position = match groups.iter().position(|(surface, _)| *surface == parent) {
                Some(position) => position,
                None => {
                    // The parent is the anchor of its sibling sequence, except the window
                    // surface whose own position is the base entry.
                    let anchor: Vec<(WlSurface, Option<WlSubsurface>)> = if parent == window {
                        Vec::new()
                    } else {
                        vec![(parent.clone(), None)]
                    };
                    groups.push((parent, anchor));
                    groups.len() - 1
                }
            };
            groups[position].1.push(node);
        }
        for (parent, siblings) in groups {
            // The anchor is the parent surface (for the window: the base entry).
            let anchor = siblings
                .iter()
                .position(|(surface, subsurface)| subsurface.is_none() && *surface == parent)
                .unwrap_or(0);
            for index in (0..anchor).rev() {
                if let Some(subsurface) = &siblings[index].1 {
                    subsurface.place_below(&siblings[index + 1].0);
                }
            }
            for index in anchor + 1..siblings.len() {
                if let Some(subsurface) = &siblings[index].1 {
                    subsurface.place_above(&siblings[index - 1].0);
                }
            }
        }
        self.stacked = Some(layout);
    }

    /// A compositor surface was recreated: native handles obtained earlier are stale.
    fn bump_generation(&self) {
        self.env.bump_generation();
    }

    /// The GPU device was recovered: overlay renderers of the lost device are discarded and
    /// recreated from the new one on their next frame, and embedders remount their content.
    pub(crate) fn device_recovered(&mut self) {
        for overlay in &mut self.overlays {
            overlay.renderer = None;
            overlay.mapped = false;
        }
        self.bump_generation();
    }

    /// The native children that must run their recreation callbacks, with the composition
    /// handles that are valid now. Called without the window state borrowed.
    pub(crate) fn take_recreation_notices(
        &mut self,
    ) -> Option<(NativeComposition, Vec<Rc<WaylandNative>>)> {
        if !self.env.recreated.take() {
            return None;
        }
        let composition = self.native_composition()?;
        let natives = self
            .natives
            .values()
            .filter_map(Weak::upgrade)
            .inspect(|native| native.state.borrow_mut().generation = composition.generation)
            .collect();
        Some((composition, natives))
    }

    /// Sets the input region of the window's overlay: the union of its hitboxes in logical
    /// pixels, empty when there are none (PLT-001 R14, R20).
    pub(crate) fn set_overlay_input_region(&mut self, region: &[Bounds<Pixels>]) {
        let Some(overlay) = self.overlays.first_mut() else {
            return;
        };
        if overlay.region.as_deref() == Some(region) {
            return;
        }
        let wl_region = self.globals.compositor.create_region(&self.globals.qh, ());
        for rect in region {
            let left = f32::from(rect.origin.x).floor() as i32;
            let top = f32::from(rect.origin.y).floor() as i32;
            let right = f32::from(rect.origin.x + rect.size.width).ceil() as i32;
            let bottom = f32::from(rect.origin.y + rect.size.height).ceil() as i32;
            wl_region.add(left, top, right - left, bottom - top);
        }
        overlay.surface.set_input_region(Some(&wl_region));
        wl_region.destroy();
        overlay.region = Some(region.to_vec());
        overlay.needs_commit = true;
    }

    /// Draws the overlay planes first and leaves their commits cached for the window surface
    /// commit that presents the base frame (PLT-001 R17, R19). `planes` pairs a surface with
    /// its scene; `None` is the window's overlay surface. `backdrop` is the finished base
    /// plane when the base surface is opaque; the backdrop-blur effects of every overlay plane
    /// then blur it (with the plane's own earlier content blended over it), and without it they
    /// blur the plane's own earlier content alone (PLT-001 R9). Other overlay planes are never
    /// part of a plane's backdrop.
    pub(crate) fn draw_planes(
        &mut self,
        base_atlas: &Arc<WgpuAtlas>,
        planes: &[(Option<SurfaceId>, &Scene)],
        backdrop: Option<&BackdropPlane>,
    ) {
        if self.env.restack.get() || self.stacked != self.layout {
            self.apply_stacking();
        }
        for index in 0..self.overlays.len() {
            let id = self.overlays[index].id;
            let scene = planes
                .iter()
                .find(|(plane, _)| match plane {
                    None => index == 0,
                    Some(plane) => id == Some(*plane),
                })
                .map(|(_, scene)| *scene);
            let drawable = scene.filter(|scene| !scene.is_empty());
            match drawable {
                Some(scene) => self.draw_overlay(index, scene, base_atlas, backdrop),
                None => self.unmap_overlay(index),
            }
        }
    }

    fn draw_overlay(
        &mut self,
        index: usize,
        scene: &Scene,
        base_atlas: &Arc<WgpuAtlas>,
        backdrop: Option<&BackdropPlane>,
    ) {
        let reusable = self.overlays[index]
            .renderer
            .as_ref()
            .is_some_and(|renderer| {
                renderer.is_reusable() && Arc::ptr_eq(renderer.sprite_atlas(), base_atlas)
            });
        if !reusable {
            // A renderer of a lost device (or of an atlas that is gone) is never reused.
            let replaced = self.overlays[index].renderer.take().is_some();
            let overlay = &self.overlays[index];
            let Some(raw) = raw_surface(&overlay.surface) else {
                log::error!("the overlay surface lost its connection");
                return;
            };
            let config = WgpuSurfaceConfig {
                size: self.device_size,
                transparent: true,
                // Prefer Mailbox: a synchronized overlay must never block on a frame callback
                // that only the window surface commit releases.
                preferred_present_mode: Some(wgpu::PresentMode::Mailbox),
            };
            match WgpuRenderer::new_sharing_atlas(
                self.gpu_context.clone(),
                &raw,
                config,
                self.compositor_gpu,
                base_atlas.clone(),
            ) {
                Ok(renderer) => self.overlays[index].renderer = Some(renderer),
                Err(error) => {
                    log::error!("creating the overlay renderer failed: {error:#}");
                    return;
                }
            }
            if replaced {
                self.bump_generation();
            }
        }
        let overlay = &mut self.overlays[index];
        if let Some(renderer) = &mut overlay.renderer {
            if renderer.draw_over(scene, backdrop) {
                overlay.mapped = true;
                overlay.needs_commit = false;
            } else {
                log::warn!("the overlay frame was not presented");
            }
        }
    }

    /// The overlay plane has nothing to show: it presents no pixels.
    fn unmap_overlay(&mut self, index: usize) {
        let overlay = &mut self.overlays[index];
        if overlay.mapped {
            overlay.surface.attach(None, 0, 0);
            overlay.mapped = false;
            overlay.needs_commit = true;
        }
        // Latches the pending state (input region) for the window surface commit.
        if overlay.needs_commit {
            overlay.surface.commit();
            overlay.needs_commit = false;
        }
    }
}

struct NativeState {
    subsurface: Option<WlSubsurface>,
    parent: WlSurface,
    /// Position relative to the parent surface, in surface coordinates.
    position: Point<i32>,
    /// The size the surface is shown at, in surface coordinates.
    destination: Option<Size<i32>>,
    visible: bool,
    /// The role object existed before and was destroyed to unmap the surface.
    unmapped_once: bool,
    generation: u64,
    callbacks: Vec<Box<dyn FnMut(NativeComposition)>>,
}

/// The Wayland side of a native or external-GPU surface: a desynchronized subsurface its
/// producer presents to independently (PLT-001 R10, R19).
pub(crate) struct WaylandNative {
    env: Rc<Env>,
    globals: Globals,
    client: WaylandClientStatePtr,
    framework_pointer: Rc<Cell<bool>>,
    origin: Rc<Cell<Point<Pixels>>>,
    surface: WlSurface,
    viewport: Option<WpViewport>,
    state: RefCell<NativeState>,
}

impl WaylandNative {
    /// Destroys the role object, which unmaps the surface.
    fn unmap(&self) {
        let mut state = self.state.borrow_mut();
        if let Some(subsurface) = state.subsurface.take() {
            subsurface.destroy();
            state.unmapped_once = true;
        }
    }

    /// Makes `parent` the surface this one is a subsurface of. Returns whether the role object
    /// had to be recreated.
    fn set_parent(&self, parent: &WlSurface) -> bool {
        let recreate = {
            let mut state = self.state.borrow_mut();
            if state.parent == *parent {
                return false;
            }
            state.parent = parent.clone();
            state.subsurface.is_some()
        };
        if recreate {
            self.unmap();
            if let Err(error) = self.map() {
                log::error!("remapping a native surface failed: {error:#}");
            }
        }
        recreate
    }

    /// Creates the role object of a visible surface.
    fn map(&self) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if state.subsurface.is_some() {
            return Ok(());
        }
        let Some(subcompositor) = self.globals.subcompositor.as_ref() else {
            bail!("the compositor does not provide wl_subcompositor");
        };
        let subsurface =
            subcompositor.get_subsurface(&self.surface, &state.parent, &self.globals.qh, ());
        // Producers present independently of the window.
        subsurface.set_desync();
        subsurface.set_position(state.position.x, state.position.y);
        state.subsurface = Some(subsurface);
        let recreated = state.unmapped_once;
        drop(state);
        self.env.restack.set(true);
        if recreated {
            // A recreated role object is a compositor-surface recreation (PLT-001 R18).
            self.env.bump_generation();
        }
        Ok(())
    }

    /// Runs the recreation callbacks with the composition handles that are valid now.
    pub(crate) fn notify_recreated(&self, composition: NativeComposition) {
        let mut callbacks = std::mem::take(&mut self.state.borrow_mut().callbacks);
        for callback in &mut callbacks {
            callback(composition);
        }
        let mut state = self.state.borrow_mut();
        callbacks.append(&mut state.callbacks);
        state.callbacks = callbacks;
    }
}

impl NativeAttachment for WaylandNative {
    fn set_geometry(
        &self,
        bounds: Bounds<DevicePixels>,
        parent_origin: Point<DevicePixels>,
    ) -> Result<()> {
        let scale = self.env.scale.get();
        let logical = |value: i32| (value as f32 / scale).round() as i32;
        let position = Point::new(
            logical(bounds.origin.x.0 - parent_origin.x.0),
            logical(bounds.origin.y.0 - parent_origin.y.0),
        );
        let destination = Size {
            width: logical(bounds.size.width.0).max(1),
            height: logical(bounds.size.height.0).max(1),
        };
        self.origin.set(Point::new(
            px(bounds.origin.x.0 as f32 / scale),
            px(bounds.origin.y.0 as f32 / scale),
        ));
        let resized = {
            let mut state = self.state.borrow_mut();
            state.position = position;
            let resized = state.destination != Some(destination);
            state.destination = Some(destination);
            if let Some(subsurface) = &state.subsurface {
                subsurface.set_position(position.x, position.y);
            }
            resized
        };
        match &self.viewport {
            Some(viewport) => viewport.set_destination(destination.width, destination.height),
            None => self.surface.set_buffer_scale(scale.round().max(1.0) as i32),
        }
        if resized {
            // The surface is desynchronized: its new size takes effect with its own commit.
            self.surface.commit();
        }
        Ok(())
    }

    fn set_visible(&self, visible: bool) -> Result<()> {
        self.state.borrow_mut().visible = visible;
        if visible {
            self.map()
        } else {
            self.unmap();
            Ok(())
        }
    }

    fn layer(&self) -> NativeLayer {
        NativeLayer::Wayland {
            surface: NonNull::new(self.surface.id().as_ptr().cast::<c_void>())
                .expect("a live wl_surface has a pointer"),
        }
    }

    fn generation(&self) -> u64 {
        self.state.borrow().generation
    }

    fn on_recreated(&self, callback: Box<dyn FnMut(NativeComposition)>) {
        self.state.borrow_mut().callbacks.push(callback);
    }

    fn set_input_forwarding(&self, forwarding: InputForwarding) {
        self.framework_pointer
            .set(forwarding == InputForwarding::Framework);
    }
}

impl Drop for WaylandNative {
    fn drop(&mut self) {
        let id: ObjectId = self.surface.id();
        self.client.unregister_subsurface(&id);
        if let Some(subsurface) = self.state.get_mut().subsurface.take() {
            subsurface.destroy();
        }
        if let Some(viewport) = &self.viewport {
            viewport.destroy();
        }
        self.surface.destroy();
    }
}
