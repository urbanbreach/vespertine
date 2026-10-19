//! The paint stream of a composed window and the scenes derived from it (PLT-001 R5-R7,
//! R14).

use std::ops::Range;

use crate::{Bounds, Pixels, Scene, UnbalancedScopes};

use super::SurfaceId;

/// A switch of the paint target surface in the paint stream: every paint operation recorded
/// from `scene_index` on belongs to `surface`, until the next switch.
///
/// A frame records one logical paint stream and a list of these switches; the frame before
/// the first switch targets the base surface. The overlay start index of a two-plane window
/// is the special case of a single switch from the base to the overlay surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceSwitch {
    /// Index into the scene's paint operations where the target changes.
    pub scene_index: usize,
    /// The surface painted from that index on.
    pub surface: SurfaceId,
}

/// Clamps an overlay start index to the number of paint operations: an index past the end
/// leaves the overlay empty, zero puts everything in the overlay.
pub fn clamp_overlay_start(overlay_start: usize, scene_len: usize) -> usize {
    overlay_start.min(scene_len)
}

/// The paint operation ranges of each surface in stream order. Stretches that target one
/// surface repeatedly keep their relative order; empty stretches are dropped.
pub fn surface_ranges(
    switches: &[SurfaceSwitch],
    base: SurfaceId,
    scene_len: usize,
) -> Vec<(SurfaceId, Range<usize>)> {
    let mut ranges = Vec::with_capacity(switches.len() + 1);
    let mut target = base;
    let mut start = 0;
    for switch in switches {
        let end = switch.scene_index.clamp(start, scene_len);
        if end > start {
            ranges.push((target, start..end));
        }
        target = switch.surface;
        start = end;
    }
    if scene_len > start {
        ranges.push((target, start..scene_len));
    }
    ranges
}

/// One drawable plane of a composed scene.
pub struct ComposedLayer {
    /// The framework-rendered surface this plane draws into.
    pub surface: SurfaceId,
    /// The plane's primitives, finished and ready to draw.
    pub scene: Scene,
    /// The bounds of the hitboxes prepared for this plane, in window-content logical pixels
    /// and already intersected with their content masks.
    pub hitboxes: Vec<Bounds<Pixels>>,
}

/// The scenes of a composed frame, one per framework-rendered surface, bottom to top in tree
/// order, plus whether the overlay captures input.
pub struct ComposedScene {
    /// Every framework-rendered surface in tree order, drawable or not.
    pub layers: Vec<ComposedLayer>,
    /// Whether the overlay captures pointer input: true when it holds deferred draws,
    /// prompts or active drag visuals, false for tooltips alone.
    pub capture: bool,
}

impl ComposedScene {
    /// Derives the per-surface scenes of a frame from its paint stream.
    ///
    /// `order` lists the framework-rendered surfaces bottom to top; operations targeting a
    /// surface that is not listed (it was removed after painting) are dropped. `hitboxes`
    /// pairs each prepared hitbox's bounds with the surface it was prepared for. Fails when a
    /// range cannot be replayed with balanced scopes; the frame must then not be drawn.
    pub fn derive(
        scene: &Scene,
        base: SurfaceId,
        switches: &[SurfaceSwitch],
        order: &[SurfaceId],
        hitboxes: &[(SurfaceId, Bounds<Pixels>)],
        capture: bool,
    ) -> Result<Self, UnbalancedScopes> {
        // Targets beyond the listed surfaces collect dropped operations.
        let discarded = order.len();
        let ranges: Vec<(usize, Range<usize>)> = surface_ranges(switches, base, scene.len())
            .into_iter()
            .map(|(surface, range)| {
                let target = order
                    .iter()
                    .position(|candidate| *candidate == surface)
                    .unwrap_or(discarded);
                (target, range)
            })
            .collect();
        let mut scenes = scene.split_ranges(&ranges, order.len() + 1)?;
        scenes.truncate(order.len());
        let layers = order
            .iter()
            .zip(scenes)
            .map(|(surface, scene)| ComposedLayer {
                surface: *surface,
                scene,
                hitboxes: hitboxes
                    .iter()
                    .filter(|(owner, _)| owner == surface)
                    .map(|(_, bounds)| *bounds)
                    .collect(),
            })
            .collect();
        Ok(ComposedScene { layers, capture })
    }

    /// The layers that draw something. Layers holding only empty scope markers are not
    /// drawable.
    pub fn drawable_layers(&self) -> impl Iterator<Item = &ComposedLayer> {
        self.layers.iter().filter(|layer| !layer.scene.is_empty())
    }

    /// The layer of a surface.
    pub fn layer(&self, surface: SurfaceId) -> Option<&ComposedLayer> {
        self.layers.iter().find(|layer| layer.surface == surface)
    }
}

/// The input region of an overlay plane: the union of its hitbox bounds while the overlay
/// captures input, and empty otherwise or when the overlay prepared no hitbox, so pointer
/// events reach the native and base surfaces below (PLT-001 R14, R20).
pub fn overlay_input_region(capture: bool, hitboxes: &[Bounds<Pixels>]) -> Vec<Bounds<Pixels>> {
    if !capture {
        return Vec::new();
    }
    hitboxes
        .iter()
        .copied()
        .filter(|bounds| !bounds.is_empty())
        .collect()
}
