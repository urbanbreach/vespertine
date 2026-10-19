//! The window side of scene layering around native child content (PLT-001): enabling, the
//! paint target, native surface positioning, frame submission, focus and input forwarding.

use std::{cell::RefCell, ops::Range, rc::Rc};

use anyhow::Result;

use crate::{
    App, Bounds, ComposedScene, CompositionController, CompositionError, CompositionShared,
    CompositionStats, NativeComposition, Pixels, PlatformInput, PointerInput, SurfaceId,
    SurfaceKind, SurfaceSwitch, Window, apply_native_state, overlay_input_region,
};

use super::{DrawPhase, PaintIndex, PrepaintStateIndex, present::PresentCallback};

/// A window's composition state. It exists once the window opted into composition; windows
/// that never did carry none and pay nothing.
pub(crate) struct WindowComposition {
    pub(crate) shared: Rc<CompositionShared>,
    /// Whether the general window composition (the surface tree) was enabled, rather than
    /// only the two-plane overlay.
    pub(crate) general: bool,
    /// The surface paint operations are currently recorded for.
    pub(crate) paint_target: SurfaceId,
    /// Whether the overlay captured input in the frame presented last.
    pub(crate) last_capture: bool,
}

impl Window {
    /// Opts this window into the two-plane layering: ordinary content renders below native
    /// children and deferred content (menus, popovers, dialogs, prompts, drag visuals,
    /// tooltips) in a transparent overlay above them.
    ///
    /// Idempotent: repeating it succeeds without effect. Platforms that cannot compose
    /// return an error describing why and the window keeps drawing a single plane exactly
    /// as before.
    pub fn enable_scene_overlay(&mut self) -> Result<()> {
        if self.composition.is_some() {
            return Ok(());
        }
        self.platform_window.enable_scene_overlay()?;
        self.install_composition(false);
        Ok(())
    }

    /// Opts this window into the general window composition: the surface tree with native,
    /// framework-rendered and external-GPU surfaces (see [`Window::composition`]). It implies
    /// the overlay plane; the two-plane layering is the default arrangement of the same tree.
    ///
    /// Idempotent. Platforms that cannot compose return an error and the window keeps
    /// drawing a single plane.
    pub fn enable_window_composition(&mut self) -> Result<()> {
        if self.composition.as_ref().is_some_and(|c| c.general) {
            return Ok(());
        }
        self.platform_window.enable_window_composition()?;
        match self.composition.as_mut() {
            Some(composition) => composition.general = true,
            None => self.install_composition(true),
        }
        Ok(())
    }

    fn install_composition(&mut self, general: bool) {
        let shared = CompositionShared::new(self.invalidator.clone());
        let base = shared.base();
        self.composition = Some(WindowComposition {
            shared,
            general,
            paint_target: base,
            last_capture: false,
        });
        // The next frame is composed.
        self.refresh();
    }

    /// The handles native producers attach to, and the generation that changes on every
    /// compositor-surface recreation (device recovery, overlay renderer recreation, X11
    /// overlay window recreation, Wayland role-object recreation). Handles obtained under an
    /// older generation are stale. `None` while composition is not enabled.
    pub fn native_composition(&self) -> Option<NativeComposition> {
        self.composition.as_ref()?;
        self.platform_window.native_composition()
    }

    /// The window composition controller: create, order, reparent, remove and query surfaces.
    /// Fails with [`CompositionError::NotEnabled`] until
    /// [`Window::enable_window_composition`] succeeded.
    pub fn composition(&self) -> Result<CompositionController<'_>, CompositionError> {
        let composition = self
            .composition
            .as_ref()
            .filter(|composition| composition.general)
            .ok_or(CompositionError::NotEnabled)?;
        Ok(CompositionController::new(
            &composition.shared,
            &*self.platform_window,
        ))
    }

    /// Carries the paint-target switches recorded inside a reused paint range into the frame
    /// being recorded: the switches keep their position relative to the replayed operations,
    /// so retained subtrees keep painting into the surfaces they painted into.
    pub(crate) fn replay_surface_switches(
        &mut self,
        range: &Range<PaintIndex>,
        replayed_scene_start: usize,
    ) {
        if self.composition.is_none() {
            return;
        }
        let from = range.start.surface_switches_index;
        let to = range.end.surface_switches_index;
        let Some(switches) = self.rendered_frame.surface_switches.get(from..to) else {
            return;
        };
        let mut target = None;
        for switch in switches {
            self.next_frame.surface_switches.push(SurfaceSwitch {
                scene_index: replayed_scene_start
                    + switch.scene_index.saturating_sub(range.start.scene_index),
                surface: switch.surface,
            });
            target = Some(switch.surface);
        }
        if let (Some(target), Some(composition)) = (target, self.composition.as_mut()) {
            composition.paint_target = target;
        }
    }

    /// The prepaint counterpart of [`Self::replay_surface_switches`] for the surfaces
    /// hitboxes are prepared for.
    pub(crate) fn replay_hitbox_switches(
        &mut self,
        range: &Range<PrepaintStateIndex>,
        replayed_hitbox_start: usize,
    ) {
        if self.composition.is_none() {
            return;
        }
        let from = range.start.hitbox_switches_index;
        let to = range.end.hitbox_switches_index;
        let Some(switches) = self.rendered_frame.hitbox_switches.get(from..to) else {
            return;
        };
        let mut target = None;
        for (index, surface) in switches {
            self.next_frame.hitbox_switches.push((
                replayed_hitbox_start + index.saturating_sub(range.start.hitboxes_index),
                *surface,
            ));
            target = Some(*surface);
        }
        if let (Some(target), Some(composition)) = (target, self.composition.as_mut()) {
            composition.paint_target = target;
        }
    }

    /// The counters of the composition work done so far (order synchronizations, attachment
    /// writes); `None` while composition is not enabled.
    pub fn composition_stats(&self) -> Option<CompositionStats> {
        self.composition
            .as_ref()
            .map(|composition| composition.shared.stats())
    }

    /// Runs paint code with `surface` as the paint target: everything painted inside goes to
    /// that framework-rendered surface. The previous target is restored afterwards, even
    /// when `f` switches targets itself. Call it while prepainting or painting.
    ///
    /// Fails, without running `f`, when composition is not enabled, the surface does not
    /// exist or it is not a framework-rendered surface.
    pub fn with_composition_surface<R>(
        &mut self,
        surface: SurfaceId,
        f: impl FnOnce(&mut Window) -> R,
    ) -> Result<R, CompositionError> {
        self.invalidator.debug_assert_paint_or_prepaint();
        let composition = self
            .composition
            .as_ref()
            .ok_or(CompositionError::NotEnabled)?;
        match composition.shared.kind(surface)? {
            SurfaceKind::Framework => {}
            kind => return Err(CompositionError::WrongKind(surface, kind)),
        }
        let previous = composition.paint_target;
        self.switch_paint_target(surface);
        let result = f(self);
        self.switch_paint_target(previous);
        Ok(result)
    }

    /// Makes `surface` the target of what is prepared or painted next, recording the switch
    /// in the frame's paint stream (paint) or hitbox list (prepaint).
    pub(crate) fn switch_paint_target(&mut self, surface: SurfaceId) {
        let phase = self.invalidator.inner.borrow().draw_phase;
        let Some(composition) = self.composition.as_mut() else {
            return;
        };
        if composition.paint_target == surface {
            return;
        }
        composition.paint_target = surface;
        match phase {
            DrawPhase::Paint => self.next_frame.surface_switches.push(SurfaceSwitch {
                scene_index: self.next_frame.scene.len(),
                surface,
            }),
            DrawPhase::Prepaint => self
                .next_frame
                .hitbox_switches
                .push((self.next_frame.hitboxes.len(), surface)),
            DrawPhase::None | DrawPhase::Focus => {}
        }
    }

    /// Starts a frame's recording: everything is painted into the base surface until a
    /// switch says otherwise.
    pub(crate) fn begin_composition_frame(&mut self) {
        if let Some(composition) = self.composition.as_mut() {
            composition.paint_target = composition.shared.base();
        }
    }

    /// Switches to the overlay surface for deferred draws, prompts, drag visuals and
    /// tooltips.
    pub(crate) fn begin_overlay(&mut self) {
        if let Some(composition) = &self.composition {
            let overlay = composition.shared.overlay();
            self.switch_paint_target(overlay);
        }
    }

    /// Moves callbacks registered while prepainting into the frame's present callbacks;
    /// paint starts.
    pub(crate) fn begin_composition_paint(&mut self) {
        let pending = std::mem::take(&mut self.next_frame.prepaint_present_callbacks);
        self.next_frame.present_callbacks.extend(pending);
        self.begin_composition_frame();
    }

    /// Whether what is being prepared or painted belongs to the overlay, which hosts native
    /// surfaces positioned now above the overlay surface.
    fn hosts_above_overlay(&self) -> bool {
        self.composition
            .as_ref()
            .is_some_and(|composition| composition.paint_target == composition.shared.overlay())
    }

    pub(crate) fn position_native_surface(
        &mut self,
        shared: Rc<CompositionShared>,
        id: SurfaceId,
        bounds: Bounds<Pixels>,
        visible: bool,
    ) {
        self.invalidator.debug_assert_paint_or_prepaint();
        let phase = self.invalidator.inner.borrow().draw_phase;
        let above_overlay = self.hosts_above_overlay();
        let device_bounds = bounds.to_device_pixels(self.scale_factor());
        let callback: PresentCallback =
            Rc::new(RefCell::new(move |window: &mut Window, _cx: &mut App| {
                apply_native_state(
                    window,
                    &shared,
                    id,
                    device_bounds,
                    bounds,
                    visible,
                    above_overlay,
                );
            }));
        match phase {
            DrawPhase::Paint => self.next_frame.present_callbacks.push(callback),
            _ => self.next_frame.prepaint_present_callbacks.push(callback),
        }
    }

    /// The bounds of the hitboxes prepared for each framework surface other than the base,
    /// in window-content logical pixels and masked like hit testing masks them.
    fn layer_hitboxes(&self) -> Vec<(SurfaceId, Bounds<Pixels>)> {
        let Some(composition) = &self.composition else {
            return Vec::new();
        };
        let base = composition.shared.base();
        let frame = &self.rendered_frame;
        let mut out = Vec::new();
        let mut target = base;
        let mut switches = frame.hitbox_switches.iter().peekable();
        for (index, hitbox) in frame.hitboxes.iter().enumerate() {
            while let Some((_, surface)) = switches.next_if(|(at, _)| *at <= index) {
                target = *surface;
            }
            if target == base {
                continue;
            }
            let bounds = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
            if !bounds.is_empty() {
                out.push((target, bounds));
            }
        }
        out
    }

    /// The overlay's input region for the rendered frame: the union of its hitbox bounds
    /// while it captures input, empty otherwise.
    pub(crate) fn overlay_region(&self) -> Vec<Bounds<Pixels>> {
        let Some(composition) = &self.composition else {
            return Vec::new();
        };
        let overlay = composition.shared.overlay();
        let hitboxes: Vec<Bounds<Pixels>> = self
            .layer_hitboxes()
            .into_iter()
            .filter(|(surface, _)| *surface == overlay)
            .map(|(_, bounds)| bounds)
            .collect();
        overlay_input_region(self.rendered_frame.overlay_capture, &hitboxes)
    }

    /// Hands the rendered frame to the platform through the layered or composed entry point
    /// (PLT-001 R5-R7, R14, R15).
    pub(crate) fn present_composition(&mut self) {
        let Some(composition) = self.composition.as_ref() else {
            return;
        };
        let shared = composition.shared.clone();
        let (base, overlay) = (shared.base(), shared.overlay());
        shared.sync_pending(&*self.platform_window);

        let capture = self.rendered_frame.overlay_capture;
        // Focus moves to the framework window when capture newly turns active over a
        // focused native child, and is not given back when capture ends. Passive tooltips
        // never capture, so they never move focus.
        let capture_started = capture && !composition.last_capture;
        if capture_started && self.platform_window.native_focus_active() {
            self.platform_window.focus_framework_window();
        }
        if let Some(composition) = self.composition.as_mut() {
            composition.last_capture = capture;
        }

        let frame = &self.rendered_frame;
        let scene = &frame.scene;
        let switches = &frame.surface_switches;
        let two_planes = shared.framework_surfaces() == [base, overlay]
            && switches.len() <= 1
            && switches.iter().all(|switch| switch.surface == overlay);
        if two_planes {
            let overlay_start = switches
                .first()
                .map_or(scene.len(), |switch| switch.scene_index);
            let region = self.overlay_region();
            self.platform_window.set_overlay_input_region(&region);
            self.platform_window
                .draw_layered(&self.rendered_frame.scene, overlay_start, capture);
            return;
        }
        let order = shared.framework_surfaces();
        let hitboxes = self.layer_hitboxes();
        match ComposedScene::derive(scene, base, switches, &order, &hitboxes, capture) {
            Ok(composed) => {
                let region = overlay_input_region(
                    capture,
                    &hitboxes
                        .iter()
                        .filter(|(surface, _)| *surface == overlay)
                        .map(|(_, bounds)| *bounds)
                        .collect::<Vec<_>>(),
                );
                self.platform_window.set_overlay_input_region(&region);
                if let Err(error) = self.platform_window.draw_composed(&composed) {
                    log::error!("drawing the composed frame failed: {error:#}");
                }
            }
            Err(error) => {
                log::error!("the frame is not drawn, its clip scopes cannot be balanced: {error}");
            }
        }
    }

    /// After framework capture ran for a mouse event: takes keyboard focus from a native
    /// child when the press landed on framework content, and forwards pointer and wheel
    /// events to cooperating windowless producers.
    pub(crate) fn forward_native_input(&mut self, event: &PlatformInput, cx: &mut App) {
        let Some(composition) = &self.composition else {
            return;
        };
        let shared = composition.shared.clone();
        if matches!(event, PlatformInput::MouseDown(_))
            && self.platform_window.native_focus_active()
        {
            self.platform_window.focus_framework_window();
        }
        if !shared.forwards_input() {
            return;
        }
        let framework_stopped = !cx.propagate_event;
        let captured = |window: &Window, position| {
            framework_stopped
                || window
                    .overlay_region()
                    .iter()
                    .any(|region| region.contains(&position))
        };
        let handled = match event {
            PlatformInput::MouseMove(move_event) => {
                let captured = captured(self, move_event.position);
                shared.forward_pointer(&PointerInput::Move(move_event.clone()), captured, self, cx)
            }
            PlatformInput::MouseDown(down) => {
                let captured = captured(self, down.position);
                shared.forward_pointer(&PointerInput::Down(down.clone()), captured, self, cx)
            }
            PlatformInput::MouseUp(up) => {
                let captured = captured(self, up.position);
                shared.forward_pointer(&PointerInput::Up(up.clone()), captured, self, cx)
            }
            PlatformInput::MouseExited(exit) => {
                shared.forward_pointer(&PointerInput::Exited(exit.clone()), true, self, cx)
            }
            PlatformInput::ScrollWheel(wheel) => {
                let captured = captured(self, wheel.position);
                shared.forward_wheel(wheel, captured, self, cx)
            }
            _ => false,
        };
        if handled {
            cx.propagate_event = false;
        }
    }
}

#[cfg(test)]
mod tests;
