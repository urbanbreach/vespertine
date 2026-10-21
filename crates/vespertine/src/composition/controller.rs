//! The window composition controller: surface tree plus platform synchronization, native
//! attachment state and input registration (PLT-001 R3, R10-R12, R16, R18).

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use anyhow::Result;

use crate::{
    App, Bounds, DevicePixels, InputForwarding, NativeAttachment, NativeComposition, NativeLayer,
    Pixels, PlatformWindow, Point, ScrollWheelEvent, Window, WindowInvalidator,
};

use super::{
    CompositionError, CompositionLayout, NativePointerEvent, PointerDelivery, PointerInput,
    PointerRouter, RouteSurface, SurfaceId, SurfaceKind, SurfaceTree,
};

/// A handler for pointer events forwarded to a cooperating windowless producer. It returns
/// whether it handled the event; the producer's surface stops further propagation of
/// pointer-down events it handled.
pub type NativePointerHandler = Box<dyn FnMut(&NativePointerEvent, &mut Window, &mut App) -> bool>;

/// A handler for wheel events forwarded to a cooperating windowless producer. It returns
/// whether it handled the event; an unhandled event reaches the next surface below.
pub type NativeWheelHandler = Box<dyn FnMut(&ScrollWheelEvent, &mut Window, &mut App) -> bool>;

/// Counters of the work the composition does, for tests and performance measurement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CompositionStats {
    /// Successful surface-order synchronizations with the platform.
    pub order_syncs: usize,
    /// Successful attachment geometry writes.
    pub bounds_writes: usize,
    /// Successful attachment visibility writes.
    pub visibility_writes: usize,
    /// Attachment writes that failed and will be retried.
    pub failed_writes: usize,
    /// Attachments created.
    pub attachments_created: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AppliedGeometry {
    bounds: Bounds<DevicePixels>,
    parent_origin: Point<DevicePixels>,
}

struct Slot {
    attachment: Rc<dyn NativeAttachment>,
    /// The geometry most recently written successfully.
    applied_geometry: Option<AppliedGeometry>,
    /// The visibility most recently written successfully.
    applied_visible: Option<bool>,
    /// The latest requested device bounds, used to derive the parent origin of children.
    device_bounds: Option<Bounds<DevicePixels>>,
    /// The latest requested logical bounds, used for input routing.
    logical_bounds: Bounds<Pixels>,
    visible: bool,
    forwarding: InputForwarding,
    pointer_handler: Option<NativePointerHandler>,
    wheel_handler: Option<NativeWheelHandler>,
    recreated: Vec<Box<dyn FnMut(NativeComposition)>>,
}

struct Inner {
    tree: SurfaceTree,
    slots: HashMap<SurfaceId, Slot>,
    stats: CompositionStats,
    /// The tree changed without the platform having been told yet (surfaces removed by
    /// dropping their handles).
    needs_sync: bool,
    router: PointerRouter,
}

/// State shared between the window, the controller and the surface handles.
pub(crate) struct CompositionShared {
    inner: RefCell<Inner>,
    invalidator: WindowInvalidator,
}

impl CompositionShared {
    pub(crate) fn new(invalidator: WindowInvalidator) -> Rc<Self> {
        Rc::new(CompositionShared {
            inner: RefCell::new(Inner {
                tree: SurfaceTree::new(),
                slots: HashMap::new(),
                stats: CompositionStats::default(),
                needs_sync: false,
                router: PointerRouter::default(),
            }),
            invalidator,
        })
    }

    pub(crate) fn base(&self) -> SurfaceId {
        self.inner.borrow().tree.base()
    }

    pub(crate) fn overlay(&self) -> SurfaceId {
        self.inner.borrow().tree.overlay()
    }

    pub(crate) fn stats(&self) -> CompositionStats {
        self.inner.borrow().stats
    }

    pub(crate) fn layout(&self) -> CompositionLayout {
        self.inner.borrow().tree.layout()
    }

    /// Whether the window needs more than the base and overlay planes to be drawn: it has a
    /// framework-rendered surface besides them.
    pub(crate) fn framework_surfaces(&self) -> Vec<SurfaceId> {
        self.inner.borrow().tree.framework_surfaces()
    }

    pub(crate) fn kind(&self, id: SurfaceId) -> Result<SurfaceKind, CompositionError> {
        self.inner.borrow().tree.kind(id)
    }

    /// Removes a surface because its owning handle was dropped. The platform learns about it
    /// at the next presentation.
    fn remove_dropped(&self, id: SurfaceId) {
        let removed = {
            let Ok(mut inner) = self.inner.try_borrow_mut() else {
                // No borrow of the composition state is held across callbacks or slot
                // destruction, so a handle cannot be dropped while it is borrowed; report it
                // rather than lose it.
                log::error!("surface {id:?} dropped while composition state is borrowed");
                return;
            };
            if inner.tree.remove(id).is_err() {
                return;
            }
            inner.router.forget(id);
            inner.needs_sync = true;
            inner.slots.remove(&id)
        };
        // The slot's handlers may own other surface handles, whose drop needs the state.
        drop(removed);
        self.invalidator.set_dirty(true);
    }

    /// Tells the platform about tree changes it has not seen (removals). Failure keeps the
    /// pending flag so the next presentation retries.
    pub(crate) fn sync_pending(&self, platform: &dyn PlatformWindow) {
        let layout = {
            let inner = self.inner.borrow();
            if !inner.needs_sync {
                return;
            }
            inner.tree.layout()
        };
        match platform.sync_composition_order(&layout) {
            Ok(()) => {
                let mut inner = self.inner.borrow_mut();
                inner.needs_sync = false;
                inner.stats.order_syncs += 1;
            }
            Err(error) => {
                log::error!("synchronizing the composition order failed, retrying: {error:#}");
                self.invalidator.set_dirty(true);
            }
        }
    }

    /// Runs `mutate` on the tree and synchronizes the platform; if synchronization fails the
    /// previous tree is restored and the error returned.
    fn mutate_tree<R>(
        &self,
        platform: &dyn PlatformWindow,
        mutate: impl FnOnce(&mut SurfaceTree) -> Result<R, CompositionError>,
    ) -> Result<R, CompositionError> {
        let (before, result, layout) = {
            let mut inner = self.inner.borrow_mut();
            let before = inner.tree.clone();
            let result = mutate(&mut inner.tree)?;
            (before, result, inner.tree.layout())
        };
        match platform.sync_composition_order(&layout) {
            Ok(()) => {
                let mut inner = self.inner.borrow_mut();
                inner.stats.order_syncs += 1;
                inner.needs_sync = false;
                Ok(result)
            }
            Err(error) => {
                let previous = before.layout();
                self.inner.borrow_mut().tree = before;
                if let Err(restore_error) = platform.sync_composition_order(&previous) {
                    log::error!(
                        "restoring the composition order after a failed change also failed: {restore_error:#}"
                    );
                }
                Err(CompositionError::Platform(error))
            }
        }
    }

    /// Creates a surface of `kind`, with a platform attachment for attachment kinds. The
    /// surface starts between the base and the overlay.
    fn create(
        self: &Rc<Self>,
        platform: &dyn PlatformWindow,
        kind: SurfaceKind,
    ) -> Result<SurfaceId, CompositionError> {
        let id = {
            let mut inner = self.inner.borrow_mut();
            inner.tree.create_below_overlay(kind)
        };
        let rollback = |this: &Self, error: CompositionError| {
            let mut inner = this.inner.borrow_mut();
            let _ = inner.tree.remove(id);
            inner.slots.remove(&id);
            error
        };
        if kind.is_attachment() {
            let attachment = match platform.create_native_attachment(id, kind) {
                Ok(attachment) => attachment,
                Err(error) => return Err(rollback(self, CompositionError::Platform(error))),
            };
            let weak = Rc::downgrade(self);
            attachment.on_recreated(Box::new(move |composition| {
                if let Some(shared) = weak.upgrade() {
                    shared.attachment_recreated(id, composition);
                }
            }));
            let mut inner = self.inner.borrow_mut();
            inner.stats.attachments_created += 1;
            inner.slots.insert(
                id,
                Slot {
                    attachment,
                    applied_geometry: None,
                    applied_visible: None,
                    device_bounds: None,
                    logical_bounds: Bounds::default(),
                    visible: false,
                    forwarding: InputForwarding::Native,
                    pointer_handler: None,
                    wheel_handler: None,
                    recreated: Vec::new(),
                },
            );
        }
        let layout = self.inner.borrow().tree.layout();
        match platform.sync_composition_order(&layout) {
            Ok(()) => {
                self.inner.borrow_mut().stats.order_syncs += 1;
                Ok(id)
            }
            Err(error) => Err(rollback(self, CompositionError::Platform(error))),
        }
    }

    /// The compositor surface behind an attachment was recreated: its applied state is gone,
    /// so the last requested geometry and visibility are written again on the next frame,
    /// and the embedder's recreation callbacks run so external content can reattach.
    fn attachment_recreated(&self, id: SurfaceId, composition: NativeComposition) {
        let mut callbacks = {
            let mut inner = self.inner.borrow_mut();
            let Some(slot) = inner.slots.get_mut(&id) else {
                return;
            };
            slot.applied_geometry = None;
            slot.applied_visible = None;
            std::mem::take(&mut slot.recreated)
        };
        for callback in &mut callbacks {
            callback(composition);
        }
        if let Some(slot) = self.inner.borrow_mut().slots.get_mut(&id) {
            callbacks.append(&mut slot.recreated);
            slot.recreated = callbacks;
        }
        self.invalidator.set_dirty(true);
    }

    /// Applies the requested state of an attachment: skipped when geometry and visibility
    /// equal the last successfully applied values; a failed write leaves the applied state
    /// unchanged so the next frame retries it.
    fn apply_attachment(
        &self,
        id: SurfaceId,
        device_bounds: Bounds<DevicePixels>,
        logical_bounds: Bounds<Pixels>,
        visible: bool,
    ) -> Result<()> {
        let (attachment, geometry, write_geometry, write_visible) = {
            let mut inner = self.inner.borrow_mut();
            let parent_origin = inner
                .tree
                .parent(id)
                .ok()
                .flatten()
                .and_then(|parent| inner.slots.get(&parent))
                .and_then(|parent| parent.device_bounds)
                .map(|bounds| bounds.origin)
                .unwrap_or_default();
            let Some(slot) = inner.slots.get_mut(&id) else {
                // Removed since the callback was recorded: nothing to apply.
                return Ok(());
            };
            slot.device_bounds = Some(device_bounds);
            slot.logical_bounds = logical_bounds;
            slot.visible = visible;
            let geometry = AppliedGeometry {
                bounds: device_bounds,
                parent_origin,
            };
            (
                slot.attachment.clone(),
                geometry,
                slot.applied_geometry != Some(geometry),
                slot.applied_visible != Some(visible),
            )
        };
        if !visible && !write_visible {
            // Hidden and already unmapped: nothing to keep in step.
        }
        let mut failure = None;
        if write_geometry {
            match attachment.set_geometry(geometry.bounds, geometry.parent_origin) {
                Ok(()) => {
                    let mut inner = self.inner.borrow_mut();
                    inner.stats.bounds_writes += 1;
                    if let Some(slot) = inner.slots.get_mut(&id) {
                        slot.applied_geometry = Some(geometry);
                    }
                }
                Err(error) => {
                    self.inner.borrow_mut().stats.failed_writes += 1;
                    failure = Some(error);
                }
            }
        }
        if write_visible && failure.is_none() {
            match attachment.set_visible(visible) {
                Ok(()) => {
                    let mut inner = self.inner.borrow_mut();
                    inner.stats.visibility_writes += 1;
                    if let Some(slot) = inner.slots.get_mut(&id) {
                        slot.applied_visible = Some(visible);
                    }
                }
                Err(error) => {
                    self.inner.borrow_mut().stats.failed_writes += 1;
                    failure = Some(error);
                }
            }
        }
        if !visible {
            self.inner.borrow_mut().router.forget(id);
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Candidates for pointer forwarding, topmost first.
    fn route_surfaces(inner: &Inner) -> Vec<RouteSurface> {
        inner
            .tree
            .layout()
            .entries
            .iter()
            .rev()
            .filter_map(|entry| {
                let slot = inner.slots.get(&entry.id)?;
                (slot.visible
                    && slot.forwarding == InputForwarding::Framework
                    && slot.pointer_handler.is_some())
                .then_some(RouteSurface {
                    id: entry.id,
                    bounds: slot.logical_bounds,
                    above_overlay: inner.tree.is_above_overlay(entry.id),
                })
            })
            .collect()
    }

    /// Routes a pointer input to cooperating producers and runs their handlers. Returns
    /// whether a handler reported a pointer-down as handled.
    pub(crate) fn forward_pointer(
        &self,
        input: &PointerInput,
        captured: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let deliveries: Vec<PointerDelivery> = {
            let mut inner = self.inner.borrow_mut();
            let surfaces = Self::route_surfaces(&inner);
            if surfaces.is_empty() && inner.router.recipient().is_none() {
                return false;
            }
            inner.router.route(input, &surfaces, captured)
        };
        let mut handled = false;
        for delivery in deliveries {
            // The handler is taken out so it can call back into the composition.
            let handler = self
                .inner
                .borrow_mut()
                .slots
                .get_mut(&delivery.surface)
                .and_then(|slot| slot.pointer_handler.take());
            let Some(mut handler) = handler else {
                continue;
            };
            let consumed = handler(&delivery.event, window, cx);
            if let Some(slot) = self.inner.borrow_mut().slots.get_mut(&delivery.surface)
                && slot.pointer_handler.is_none()
            {
                slot.pointer_handler = Some(handler);
            }
            handled |= consumed && matches!(delivery.event, NativePointerEvent::Down(_));
        }
        handled
    }

    /// Forwards a wheel event to the surfaces under the pointer, topmost first, until one
    /// handles it. Returns whether it was handled.
    pub(crate) fn forward_wheel(
        &self,
        event: &ScrollWheelEvent,
        captured: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        let targets = {
            let inner = self.inner.borrow();
            let surfaces: Vec<RouteSurface> = Self::route_surfaces(&inner)
                .into_iter()
                .filter(|surface| {
                    inner
                        .slots
                        .get(&surface.id)
                        .is_some_and(|slot| slot.wheel_handler.is_some())
                })
                .collect();
            inner.router.wheel_targets(event, &surfaces, captured)
        };
        for target in targets {
            let handler = self
                .inner
                .borrow_mut()
                .slots
                .get_mut(&target)
                .and_then(|slot| slot.wheel_handler.take());
            let Some(mut handler) = handler else {
                continue;
            };
            let handled = handler(event, window, cx);
            if let Some(slot) = self.inner.borrow_mut().slots.get_mut(&target)
                && slot.wheel_handler.is_none()
            {
                slot.wheel_handler = Some(handler);
            }
            if handled {
                return true;
            }
        }
        false
    }

    /// Whether any visible surface listens to framework-forwarded pointer input.
    pub(crate) fn forwards_input(&self) -> bool {
        let inner = self.inner.borrow();
        inner.router.recipient().is_some() || !Self::route_surfaces(&inner).is_empty()
    }
}

/// An owning handle to a composition surface. Dropping it removes the surface; its children
/// move to its former sibling position.
pub struct SurfaceHandle {
    id: SurfaceId,
    shared: Rc<CompositionShared>,
}

impl SurfaceHandle {
    /// The stable, opaque identity of the surface.
    pub fn id(&self) -> SurfaceId {
        self.id
    }

    /// What produces the surface's pixels.
    pub fn kind(&self) -> Result<SurfaceKind, CompositionError> {
        self.shared.kind(self.id)
    }
}

impl Drop for SurfaceHandle {
    fn drop(&mut self) {
        self.shared.remove_dropped(self.id);
    }
}

impl std::fmt::Debug for SurfaceHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SurfaceHandle").field(&self.id).finish()
    }
}

/// An owning handle to a native or external-GPU surface, with its attachment interface.
///
/// Embedders position it from paint code with [`NativeSurface::position`]; the platform
/// writes happen inside the presentation callback of the frame that painted the position.
pub struct NativeSurface {
    handle: SurfaceHandle,
}

impl NativeSurface {
    /// The surface identity.
    pub fn id(&self) -> SurfaceId {
        self.handle.id
    }

    fn slot_attachment(&self) -> Option<Rc<dyn NativeAttachment>> {
        self.handle
            .shared
            .inner
            .borrow()
            .slots
            .get(&self.handle.id)
            .map(|slot| slot.attachment.clone())
    }

    /// The opaque platform handle the producer presents to. It becomes stale when the
    /// generation changes; see [`Self::on_recreated`].
    pub fn layer(&self) -> Option<NativeLayer> {
        self.slot_attachment().map(|attachment| attachment.layer())
    }

    /// The generation of [`Self::layer`].
    pub fn generation(&self) -> Option<u64> {
        self.slot_attachment()
            .map(|attachment| attachment.generation())
    }

    /// Registers a callback run when the compositor surface behind this attachment is
    /// recreated. The previously obtained handle is stale; the callback receives the handles
    /// to remount external content with, without recreating application state.
    pub fn on_recreated(&self, callback: impl FnMut(NativeComposition) + 'static) {
        if let Some(slot) = self
            .handle
            .shared
            .inner
            .borrow_mut()
            .slots
            .get_mut(&self.handle.id)
        {
            slot.recreated.push(Box::new(callback));
        }
    }

    /// Sets whether the framework forwards pointer input over this surface to the handlers
    /// registered with [`Self::set_pointer_handler`] and [`Self::set_wheel_handler`]. The
    /// default is [`InputForwarding::Native`]: OS-managed child surfaces keep their native
    /// event path.
    pub fn set_input_forwarding(&self, forwarding: InputForwarding) {
        let attachment = {
            let mut inner = self.handle.shared.inner.borrow_mut();
            let Some(slot) = inner.slots.get_mut(&self.handle.id) else {
                return;
            };
            slot.forwarding = forwarding;
            slot.attachment.clone()
        };
        attachment.set_input_forwarding(forwarding);
    }

    /// Registers the pointer handler of a cooperating windowless producer. Events run after
    /// framework capture, only while the surface is visible and the pointer is inside its
    /// current bounds (or the surface owns a press), and need
    /// [`InputForwarding::Framework`].
    pub fn set_pointer_handler(
        &self,
        handler: impl FnMut(&NativePointerEvent, &mut Window, &mut App) -> bool + 'static,
    ) {
        if let Some(slot) = self
            .handle
            .shared
            .inner
            .borrow_mut()
            .slots
            .get_mut(&self.handle.id)
        {
            slot.pointer_handler = Some(Box::new(handler));
        }
    }

    /// Registers the wheel handler of a cooperating windowless producer.
    pub fn set_wheel_handler(
        &self,
        handler: impl FnMut(&ScrollWheelEvent, &mut Window, &mut App) -> bool + 'static,
    ) {
        if let Some(slot) = self
            .handle
            .shared
            .inner
            .borrow_mut()
            .slots
            .get_mut(&self.handle.id)
        {
            slot.wheel_handler = Some(Box::new(handler));
        }
    }

    /// Converts a pointer position reported local to this surface (including while the
    /// producer holds a grab) to window-content logical pixels by adding the surface's
    /// current origin.
    pub fn window_position(&self, local: Point<Pixels>) -> Point<Pixels> {
        let origin = self
            .handle
            .shared
            .inner
            .borrow()
            .slots
            .get(&self.handle.id)
            .map(|slot| slot.logical_bounds.origin)
            .unwrap_or_default();
        super::slot_local_to_window(origin, local)
    }

    /// Positions the surface and sets its visibility for the frame being prepared. Call it
    /// while prepainting or painting; `bounds` are in window-content logical pixels.
    ///
    /// The platform write happens inside a [`Window::on_present`] callback, so it becomes
    /// visible together with the framework buffer of the same frame, and is skipped when
    /// bounds and visibility equal the last successfully applied values. A surface positioned
    /// while a deferred overlay is being prepared or painted is hosted above the overlay
    /// surface (so it can sit inside a dialog); positioning it again in base content returns
    /// it below the overlay. An attachment hosted above the overlay can cover later
    /// framework content painted on the same overlay: keep such content out of its bounds.
    pub fn position(&self, window: &mut Window, bounds: Bounds<Pixels>, visible: bool) {
        window.position_native_surface(self.handle.shared.clone(), self.handle.id, bounds, visible);
    }
}

impl std::fmt::Debug for NativeSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("NativeSurface")
            .field(&self.handle.id)
            .finish()
    }
}

/// Applies a native surface's requested state during presentation. Called by the callback
/// [`Window::position_native_surface`] registers.
pub(crate) fn apply_native_state(
    window: &mut Window,
    shared: &Rc<CompositionShared>,
    id: SurfaceId,
    device_bounds: Bounds<DevicePixels>,
    logical_bounds: Bounds<Pixels>,
    visible: bool,
    above_overlay: bool,
) {
    let mut retry = false;
    if let Ok(controller) = window.composition()
        && let Err(error) = controller.host_above_overlay(id, above_overlay)
    {
        log::error!("changing the hosting level of surface {id:?} failed: {error}");
        retry = true;
    }
    if let Err(error) = shared.apply_attachment(id, device_bounds, logical_bounds, visible) {
        log::error!("applying native surface {id:?} state failed, retrying next frame: {error:#}");
        retry = true;
    }
    if retry {
        window.refresh();
    }
}

/// The window's composition controller: creates, orders, reparents, removes and queries
/// surfaces. Every change of the surface tree is synchronized with the platform; if that
/// fails the previous tree is restored and the error returned.
///
/// Obtain it with [`Window::composition`](crate::Window::composition) after enabling
/// composition.
pub struct CompositionController<'a> {
    shared: &'a Rc<CompositionShared>,
    platform: &'a dyn PlatformWindow,
}

impl<'a> CompositionController<'a> {
    pub(crate) fn new(shared: &'a Rc<CompositionShared>, platform: &'a dyn PlatformWindow) -> Self {
        CompositionController { shared, platform }
    }

    /// The base surface: the window's own content.
    pub fn base(&self) -> SurfaceId {
        self.shared.base()
    }

    /// The overlay surface: deferred framework content above the native children.
    pub fn overlay(&self) -> SurfaceId {
        self.shared.overlay()
    }

    /// Creates a native surface between the base and the overlay.
    pub fn create_native_surface(&self) -> Result<NativeSurface, CompositionError> {
        self.create_attachment(SurfaceKind::Native)
    }

    /// Creates an external-GPU surface between the base and the overlay. It is composed
    /// exactly like a native surface; its producer renders with the window's GPU device.
    pub fn create_external_gpu_surface(&self) -> Result<NativeSurface, CompositionError> {
        self.create_attachment(SurfaceKind::ExternalGpu)
    }

    fn create_attachment(&self, kind: SurfaceKind) -> Result<NativeSurface, CompositionError> {
        let id = self.shared.create(self.platform, kind)?;
        Ok(NativeSurface {
            handle: SurfaceHandle {
                id,
                shared: self.shared.clone(),
            },
        })
    }

    /// Creates an additional framework-rendered surface between the base and the overlay.
    /// Paint into it with
    /// [`Window::with_composition_surface`](crate::Window::with_composition_surface).
    pub fn create_framework_surface(&self) -> Result<SurfaceHandle, CompositionError> {
        let id = self.shared.create(self.platform, SurfaceKind::Framework)?;
        Ok(SurfaceHandle {
            id,
            shared: self.shared.clone(),
        })
    }

    /// Places `id` directly above its sibling.
    pub fn order_above(&self, id: SurfaceId, sibling: SurfaceId) -> Result<(), CompositionError> {
        self.shared
            .mutate_tree(self.platform, |tree| tree.place_relative(id, sibling, true))
    }

    /// Places `id` directly below its sibling.
    pub fn order_below(&self, id: SurfaceId, sibling: SurfaceId) -> Result<(), CompositionError> {
        self.shared.mutate_tree(self.platform, |tree| {
            tree.place_relative(id, sibling, false)
        })
    }

    /// Moves `id` under `parent` (`None` for root level), on top of its new siblings.
    pub fn reparent(
        &self,
        id: SurfaceId,
        parent: Option<SurfaceId>,
    ) -> Result<(), CompositionError> {
        self.shared
            .mutate_tree(self.platform, |tree| tree.reparent(id, parent))
    }

    /// Removes a surface; its children take its place under its former parent. The base and
    /// overlay surfaces cannot be removed. Dropping the owning handle removes the surface
    /// too, and any handle to a removed surface is stale.
    pub fn remove(&self, id: SurfaceId) -> Result<(), CompositionError> {
        self.shared.mutate_tree(self.platform, |tree| {
            tree.remove(id)?;
            Ok(())
        })?;
        let removed = {
            let mut inner = self.shared.inner.borrow_mut();
            inner.router.forget(id);
            inner.slots.remove(&id)
        };
        // The slot's handlers may own other surface handles, whose drop needs the state.
        drop(removed);
        Ok(())
    }

    /// The kind of a surface.
    pub fn kind(&self, id: SurfaceId) -> Result<SurfaceKind, CompositionError> {
        self.shared.kind(id)
    }

    /// The parent of a surface, `None` for root-level surfaces.
    pub fn parent(&self, id: SurfaceId) -> Result<Option<SurfaceId>, CompositionError> {
        self.shared.inner.borrow().tree.parent(id)
    }

    /// The children of a surface, bottom to top.
    pub fn children(&self, id: SurfaceId) -> Result<Vec<SurfaceId>, CompositionError> {
        Ok(self.shared.inner.borrow().tree.children(id)?.to_vec())
    }

    /// The whole tree flattened to stacking order, bottom to top.
    pub fn layout(&self) -> CompositionLayout {
        self.shared.layout()
    }

    /// The counters of the work done so far.
    pub fn stats(&self) -> CompositionStats {
        self.shared.stats()
    }

    /// Moves a root-level surface directly above (`above`) or below the overlay surface when
    /// it is not already hosted there. A surface nested under another follows its ancestor
    /// and is left alone. Only an actual change synchronizes the platform.
    pub(crate) fn host_above_overlay(
        &self,
        id: SurfaceId,
        above: bool,
    ) -> Result<(), CompositionError> {
        let overlay = self.overlay();
        let needs_move = {
            let inner = self.shared.inner.borrow();
            match inner.tree.parent(id) {
                Ok(None) => inner.tree.is_above_overlay(id) != above,
                _ => false,
            }
        };
        if !needs_move {
            return Ok(());
        }
        self.shared.mutate_tree(self.platform, |tree| {
            tree.place_relative(id, overlay, above)
        })
    }
}
