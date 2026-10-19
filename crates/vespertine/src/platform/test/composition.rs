//! A layered test window (PLT-001): records layered and composed draws, attachment writes and
//! order synchronizations so composition can be tested without a display server.
#![cfg_attr(not(test), allow(dead_code))]

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use anyhow::{Result, bail};

use crate::{
    Bounds, ComposedScene, CompositionLayout, DevicePixels, InputForwarding, NativeAttachment,
    NativeComposition, NativeDevice, NativeLayer, Pixels, Point, Scene, SurfaceId, SurfaceKind,
    TestWindow, UnbalancedScopes,
};

/// One plane of a recorded layered draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PlaneRecord {
    /// Paint operations in the plane (scope markers included).
    pub(crate) operations: usize,
    /// Whether the plane holds anything drawable.
    pub(crate) drawable: bool,
}

/// A recorded [`PlatformWindow::draw_layered`](crate::PlatformWindow::draw_layered) call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayeredDraw {
    /// The overlay start index the window passed.
    pub(crate) requested_start: usize,
    /// The index after clamping to the scene length.
    pub(crate) overlay_start: usize,
    /// The capture flag the window passed.
    pub(crate) capture: bool,
    /// Paint operations of the whole scene.
    pub(crate) scene_operations: usize,
    pub(crate) base: PlaneRecord,
    pub(crate) overlay: PlaneRecord,
    /// The frame was not drawn because its scopes cannot be balanced.
    pub(crate) unbalanced: Option<UnbalancedScopes>,
    /// The generation of the device the frame was drawn on.
    pub(crate) generation: u64,
}

/// A recorded composed draw: the drawable state of every framework layer in tree order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ComposedDraw {
    pub(crate) layers: Vec<(SurfaceId, PlaneRecord, Vec<Bounds<Pixels>>)>,
    pub(crate) capture: bool,
}

/// What the test platform recorded of one native attachment.
#[derive(Default)]
pub(crate) struct AttachmentState {
    pub(crate) geometry_writes: Vec<(Bounds<DevicePixels>, Point<DevicePixels>)>,
    pub(crate) visibility_writes: Vec<bool>,
    /// The next this many geometry writes fail.
    pub(crate) fail_geometry_writes: usize,
    pub(crate) forwarding: InputForwarding,
    recreated: Vec<Box<dyn FnMut(NativeComposition)>>,
    layer: u64,
    generation: u64,
}

impl AttachmentState {
    /// The generation of the attachment's handle.
    pub(crate) fn generation_value(&self) -> u64 {
        self.generation
    }
}

/// The attachment the test platform hands the framework.
pub(crate) struct TestAttachment {
    pub(crate) state: Rc<RefCell<AttachmentState>>,
}

impl NativeAttachment for TestAttachment {
    fn set_geometry(
        &self,
        bounds: Bounds<DevicePixels>,
        parent_origin: Point<DevicePixels>,
    ) -> Result<()> {
        let mut state = self.state.borrow_mut();
        if state.fail_geometry_writes > 0 {
            state.fail_geometry_writes -= 1;
            bail!("test geometry write failed");
        }
        state.geometry_writes.push((bounds, parent_origin));
        Ok(())
    }

    fn set_visible(&self, visible: bool) -> Result<()> {
        self.state.borrow_mut().visibility_writes.push(visible);
        Ok(())
    }

    fn layer(&self) -> NativeLayer {
        NativeLayer::Test {
            id: self.state.borrow().layer,
        }
    }

    fn generation(&self) -> u64 {
        self.state.borrow().generation
    }

    fn on_recreated(&self, callback: Box<dyn FnMut(NativeComposition)>) {
        self.state.borrow_mut().recreated.push(callback);
    }

    fn set_input_forwarding(&self, forwarding: InputForwarding) {
        self.state.borrow_mut().forwarding = forwarding;
    }
}

/// The test window's composition state.
#[derive(Default)]
pub(crate) struct TestCompositionState {
    /// Whether the platform provides composition at all; the in-memory platform does not
    /// unless a test opts in.
    pub(crate) available: bool,
    pub(crate) overlay_enabled: bool,
    pub(crate) window_enabled: bool,
    pub(crate) generation: u64,
    pub(crate) device_lost: bool,
    pub(crate) layered_draws: Vec<LayeredDraw>,
    pub(crate) composed_draws: Vec<ComposedDraw>,
    pub(crate) skipped_while_lost: usize,
    pub(crate) input_regions: Vec<Vec<Bounds<Pixels>>>,
    pub(crate) order_syncs: Vec<CompositionLayout>,
    /// The next this many order synchronizations fail.
    pub(crate) fail_order_syncs: usize,
    pub(crate) attachments: HashMap<SurfaceId, Rc<RefCell<AttachmentState>>>,
    pub(crate) native_focus: bool,
    pub(crate) framework_focus_requests: usize,
    /// The device generation the overlay renderer was created on.
    pub(crate) overlay_renderer: Option<u64>,
    pub(crate) overlay_renderer_creations: usize,
    next_layer: u64,
}

impl TestCompositionState {
    pub(crate) fn enable(&mut self, window: bool) -> Result<()> {
        if !self.available {
            bail!("window composition is not available on the test platform");
        }
        if window {
            self.window_enabled = true;
        }
        self.overlay_enabled = true;
        Ok(())
    }

    pub(crate) fn native_composition(&self) -> Option<NativeComposition> {
        (self.overlay_enabled).then_some(NativeComposition {
            device: NativeDevice::Test { id: 1 },
            layer: NativeLayer::Test { id: 0 },
            generation: self.generation,
        })
    }

    pub(crate) fn create_attachment(
        &mut self,
        surface: SurfaceId,
        _kind: SurfaceKind,
    ) -> Result<Rc<dyn NativeAttachment>> {
        if !self.window_enabled {
            bail!("window composition is not enabled");
        }
        self.next_layer += 1;
        let state = Rc::new(RefCell::new(AttachmentState {
            layer: self.next_layer,
            generation: self.generation,
            ..Default::default()
        }));
        self.attachments.insert(surface, state.clone());
        Ok(Rc::new(TestAttachment { state }))
    }

    pub(crate) fn sync_order(&mut self, layout: &CompositionLayout) -> Result<()> {
        if self.fail_order_syncs > 0 {
            self.fail_order_syncs -= 1;
            bail!("test order synchronization failed");
        }
        self.attachments
            .retain(|surface, _| layout.entry(*surface).is_some());
        self.order_syncs.push(layout.clone());
        Ok(())
    }

    /// Makes sure an overlay renderer exists for the current device; a renderer created on
    /// an older (lost) device is discarded, never reused.
    fn ensure_overlay_renderer(&mut self) {
        if self.overlay_renderer != Some(self.generation) {
            self.overlay_renderer = Some(self.generation);
            self.overlay_renderer_creations += 1;
        }
    }

    pub(crate) fn draw_layered(&mut self, scene: &Scene, overlay_start: usize, capture: bool) {
        if self.device_lost {
            self.skipped_while_lost += 1;
            return;
        }
        let start = crate::clamp_overlay_start(overlay_start, scene.len());
        self.ensure_overlay_renderer();
        let mut record = LayeredDraw {
            requested_start: overlay_start,
            overlay_start: start,
            capture,
            scene_operations: scene.len(),
            base: PlaneRecord::default(),
            overlay: PlaneRecord::default(),
            unbalanced: None,
            generation: self.generation,
        };
        match scene.split_at_overlay(start) {
            Ok((base, overlay)) => {
                record.base = PlaneRecord {
                    operations: base.len(),
                    drawable: !base.is_empty(),
                };
                record.overlay = PlaneRecord {
                    operations: overlay.len(),
                    drawable: !overlay.is_empty(),
                };
            }
            Err(error) => record.unbalanced = Some(error),
        }
        self.layered_draws.push(record);
    }

    pub(crate) fn draw_composed(&mut self, composed: &ComposedScene) {
        self.ensure_overlay_renderer();
        self.composed_draws.push(ComposedDraw {
            layers: composed
                .layers
                .iter()
                .map(|layer| {
                    (
                        layer.surface,
                        PlaneRecord {
                            operations: layer.scene.len(),
                            drawable: !layer.scene.is_empty(),
                        },
                        layer.hitboxes.clone(),
                    )
                })
                .collect(),
            capture: composed.capture,
        });
    }
}

/// The test window's composition controls and observations.
impl TestWindow {
    /// Lets the window's composition succeed (the in-memory platform has none by default).
    pub(crate) fn allow_composition(&self) {
        self.0.lock().composition.available = true;
    }

    pub(crate) fn layered_draws(&self) -> Vec<LayeredDraw> {
        self.0.lock().composition.layered_draws.clone()
    }

    pub(crate) fn composed_draws(&self) -> Vec<ComposedDraw> {
        self.0.lock().composition.composed_draws.clone()
    }

    pub(crate) fn input_regions(&self) -> Vec<Vec<Bounds<Pixels>>> {
        self.0.lock().composition.input_regions.clone()
    }

    pub(crate) fn last_layout(&self) -> Option<CompositionLayout> {
        self.0.lock().composition.order_syncs.last().cloned()
    }

    pub(crate) fn fail_next_order_syncs(&self, count: usize) {
        self.0.lock().composition.fail_order_syncs = count;
    }

    pub(crate) fn attachment(&self, surface: SurfaceId) -> Option<Rc<RefCell<AttachmentState>>> {
        self.0.lock().composition.attachments.get(&surface).cloned()
    }

    pub(crate) fn set_native_focus(&self, focused: bool) {
        self.0.lock().composition.native_focus = focused;
    }

    pub(crate) fn native_focus(&self) -> bool {
        self.0.lock().composition.native_focus
    }

    pub(crate) fn framework_focus_requests(&self) -> usize {
        self.0.lock().composition.framework_focus_requests
    }

    pub(crate) fn overlay_renderer_creations(&self) -> usize {
        self.0.lock().composition.overlay_renderer_creations
    }

    pub(crate) fn overlay_renderer_generation(&self) -> Option<u64> {
        self.0.lock().composition.overlay_renderer
    }

    /// The graphics device is lost: frames are not drawn until it is recovered.
    pub(crate) fn simulate_device_loss(&self) {
        self.0.lock().composition.device_lost = true;
    }

    /// The compositor surface is recreated (device recovery): the generation changes and
    /// every attachment reports the recreation. Renderers created on the previous device are
    /// stale.
    pub(crate) fn simulate_composition_recreation(&self) {
        let (generation, recreated) = {
            let mut lock = self.0.lock();
            lock.composition.device_lost = false;
            lock.composition.generation += 1;
            let generation = lock.composition.generation;
            let states: Vec<_> = lock.composition.attachments.values().cloned().collect();
            (generation, states)
        };
        for state in recreated {
            let mut callbacks = {
                let mut state = state.borrow_mut();
                state.generation = generation;
                std::mem::take(&mut state.recreated)
            };
            let composition = NativeComposition {
                device: NativeDevice::Test { id: 1 },
                layer: NativeLayer::Test {
                    id: state.borrow().layer,
                },
                generation,
            };
            for callback in &mut callbacks {
                callback(composition);
            }
            let mut state = state.borrow_mut();
            callbacks.append(&mut state.recreated);
            state.recreated = callbacks;
        }
    }
}
