//! Paint scopes that attenuate or cut everything painted inside them per fragment.
//!
//! Three mechanisms share the data types in this module:
//!
//! * the scoped *edge fade* ([`EdgeFade`], [`crate::Window::with_edge_fade`]), which fades
//!   the alpha of content as it approaches selected edges of a window-space region,
//! * the scoped *rounded clip* ([`crate::Window::with_rounded_clip`]), which cuts content
//!   along one uniform-radius rounded rectangle, and
//! * the opt-in *curved overflow clip* of elements ([`crate::Styled::curved_overflow_clip`]),
//!   which cuts descendants along an element's own curved padding edge.
//!
//! The first two travel with every primitive as a [`ScopeParams`] block; the third is a
//! chain of [`CurvedClipShape`] records in the scene. The functions on these types are the
//! reference implementation of the arithmetic the GPU shaders evaluate per fragment, and are
//! used by tests to check rendered pixels.
//!
//! # Documented limits of the explicit scopes
//!
//! * A quad whose fill is a single solid color, with square corners, no border and no
//!   active fade edge (a simple solid quad) bypasses the rounded-clip coverage entirely and
//!   is cut only by the rectangular content mask when every quad of its draw batch is also a
//!   simple solid quad (RND-002 R6, PRF-008 R3); in a batch that also holds any other quad
//!   it is rounded like the rest. Software adapters additionally shade the interior of large
//!   bordered, rounded or faded solid quads by the plain solid route. Force the general route
//!   (gradient fill, nonzero radius or border, or an enabled fade edge) when a rounded cut
//!   is required, and never rely on the rounded scope to leave such a fill square.
//! * Paths, underlines, strikethroughs, drop and inset shadows and external surfaces keep
//!   only the rectangular content mask under the explicit rounded clip (RND-002 R7).
//! * Paths, shadows and decorations are faded conservatively, by one scalar computed from
//!   the primitive's logical bounding rectangle (RND-001 R12); a path or shadow whose box
//!   touches an active fade edge is fully transparent.

use crate::{Bounds, Pixels};

/// Identifier of a curved-clip chain node in a [`crate::Scene`]; `0` means "no curved clip".
pub type CurvedClipId = u32;

/// Per-primitive scope parameters (device pixels), carried by quads and sprites.
///
/// A zeroed block is the neutral state: no active fade band, inactive rounded clip, no
/// curved-clip chain.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub struct ScopeParams {
    /// Fade edge coordinates: top, bottom, left, right.
    pub fade_edges: [f32; 4],
    /// Fade band widths: top, bottom, left, right. A width that is not positive disables
    /// the edge.
    pub fade_bands: [f32; 4],
    /// Rounded clip bounds: left, top, right, bottom. Inactive unless right > left.
    pub clip_bounds: [f32; 4],
    /// Rounded clip radius.
    pub clip_radius: f32,
    /// Chain of curved overflow clips applying to this primitive (stamped by the scene).
    pub curved_clip: CurvedClipId,
    /// Reserved; keeps the block at 64 bytes so primitive records stay 16-byte multiples.
    pub reserved: [u32; 2],
}

impl ScopeParams {
    /// Whether any fade edge has a positive band.
    pub fn fade_active(&self) -> bool {
        self.fade_bands.iter().any(|band| *band > 0.0)
    }

    /// Whether the rounded clip is active.
    pub fn rounded_clip_active(&self) -> bool {
        self.clip_bounds[2] > self.clip_bounds[0]
    }

    /// Whether the fade or rounded clip changes any fragment.
    pub fn is_active(&self) -> bool {
        self.fade_active() || self.rounded_clip_active()
    }

    /// Squared-minimum edge coverage `E` at fragment position `p` (device pixels).
    pub fn edge_coverage(&self, p: [f32; 2]) -> f32 {
        let [top, bottom, left, right] = self.fade_edges;
        let [band_top, band_bottom, band_left, band_right] = self.fade_bands;
        let mut u = 1.0f32;
        if band_top > 0.0 {
            u = u.min(((p[1] - top) / band_top).clamp(0.0, 1.0));
        }
        if band_bottom > 0.0 {
            u = u.min(((bottom - p[1]) / band_bottom).clamp(0.0, 1.0));
        }
        if band_left > 0.0 {
            u = u.min(((p[0] - left) / band_left).clamp(0.0, 1.0));
        }
        if band_right > 0.0 {
            u = u.min(((right - p[0]) / band_right).clamp(0.0, 1.0));
        }
        u * u
    }

    /// Rounded-clip coverage `K` at fragment position `p` (device pixels).
    pub fn rounded_coverage(&self, p: [f32; 2]) -> f32 {
        if !self.rounded_clip_active() {
            return 1.0;
        }
        let [left, top, right, bottom] = self.clip_bounds;
        let half = [(right - left) / 2.0, (bottom - top) / 2.0];
        let center = [left + half[0], top + half[1]];
        let radius = self.clip_radius.min(half[0]).min(half[1]);
        let q = [
            (p[0] - center[0]).abs() - half[0] + radius,
            (p[1] - center[1]).abs() - half[1] + radius,
        ];
        let outside = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt();
        let distance = outside + q[0].max(q[1]).min(0.0) - radius;
        (0.5 - distance).clamp(0.0, 1.0)
    }

    /// The scope factor `F = E × K` at fragment position `p` (device pixels).
    pub fn factor(&self, p: [f32; 2]) -> f32 {
        self.edge_coverage(p) * self.rounded_coverage(p)
    }
}

/// An edge fade scope: content painted inside it fades toward the enabled edges of
/// `bounds`. See [`crate::Window::with_edge_fade`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EdgeFade {
    /// Fade region in window logical pixels.
    pub bounds: Bounds<Pixels>,
    /// Whether the left edge fades.
    pub left: bool,
    /// Whether the right edge fades.
    pub right: bool,
    /// Whether the top edge fades.
    pub top: bool,
    /// Whether the bottom edge fades.
    pub bottom: bool,
    /// Common band width in logical pixels.
    pub band: Pixels,
    /// Optional band override for the left edge.
    pub band_left: Option<Pixels>,
    /// Optional band override for the right edge.
    pub band_right: Option<Pixels>,
    /// Optional band override for the top edge.
    pub band_top: Option<Pixels>,
    /// Optional band override for the bottom edge.
    pub band_bottom: Option<Pixels>,
}

impl EdgeFade {
    /// A fade over `bounds` with a common `band` and no enabled edge.
    pub fn new(bounds: Bounds<Pixels>, band: Pixels) -> Self {
        Self {
            bounds,
            left: false,
            right: false,
            top: false,
            bottom: false,
            band,
            band_left: None,
            band_right: None,
            band_top: None,
            band_bottom: None,
        }
    }

    /// Whether at least one edge is enabled.
    pub fn is_enabled(&self) -> bool {
        self.top || self.bottom || self.left || self.right
    }

    /// Effective logical band widths (top, bottom, left, right); an enabled edge's band is
    /// its override or the common band raised to at least one logical pixel, a disabled
    /// edge's band is zero.
    pub fn effective_bands(&self) -> [f32; 4] {
        let band = |enabled: bool, override_band: Option<Pixels>| {
            if enabled {
                override_band.unwrap_or(self.band).0.max(1.0)
            } else {
                0.0
            }
        };
        [
            band(self.top, self.band_top),
            band(self.bottom, self.band_bottom),
            band(self.left, self.band_left),
            band(self.right, self.band_right),
        ]
    }

    /// Conservative uniform fade of a primitive whose logical bounding rectangle is
    /// `rect` (RND-001 R12): for each enabled edge the inward distance of the rectangle's
    /// nearest point divided by the band, clamped to `[0, 1]`, combined by minimum and
    /// squared.
    pub fn conservative_factor(&self, rect: Bounds<Pixels>) -> f32 {
        let [band_top, band_bottom, band_left, band_right] = self.effective_bands();
        let mut u = 1.0f32;
        if band_top > 0.0 {
            u = u.min(((rect.top() - self.bounds.top()).0 / band_top).clamp(0.0, 1.0));
        }
        if band_bottom > 0.0 {
            u = u.min(((self.bounds.bottom() - rect.bottom()).0 / band_bottom).clamp(0.0, 1.0));
        }
        if band_left > 0.0 {
            u = u.min(((rect.left() - self.bounds.left()).0 / band_left).clamp(0.0, 1.0));
        }
        if band_right > 0.0 {
            u = u.min(((self.bounds.right() - rect.right()).0 / band_right).clamp(0.0, 1.0));
        }
        u * u
    }

    /// The device-space scope fade fields for `scale_factor`, replacing any outer fade.
    pub(crate) fn scaled(&self, scale_factor: f32, scope: &mut ScopeParams) {
        let bands = self.effective_bands();
        scope.fade_edges = [
            self.bounds.top().0 * scale_factor,
            self.bounds.bottom().0 * scale_factor,
            self.bounds.left().0 * scale_factor,
            self.bounds.right().0 * scale_factor,
        ];
        scope.fade_bands = bands.map(|band| band * scale_factor);
    }
}

/// One curved overflow clip shape in device pixels (48 bytes on the GPU).
///
/// The shape is the element's border box inset by its border widths, with independent
/// elliptical radii per corner, in the order top-left, top-right, bottom-right,
/// bottom-left.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C)]
pub struct CurvedClipShape {
    /// Left, top, right, bottom.
    pub bounds: [f32; 4],
    /// Horizontal radius of each corner.
    pub radii_x: [f32; 4],
    /// Vertical radius of each corner.
    pub radii_y: [f32; 4],
}

impl CurvedClipShape {
    /// Builds the shape for an element's padding edge from its border box, outer corner
    /// radii (top-left, top-right, bottom-right, bottom-left) and border widths
    /// (top, right, bottom, left), all in device pixels (RND-002 R12).
    pub fn from_padding_edge(
        border_box: Bounds<crate::ScaledPixels>,
        radii: [f32; 4],
        widths: [f32; 4],
    ) -> Self {
        let [top, right, bottom, left] = widths;
        let origin_left = border_box.left().0 + left;
        let origin_top = border_box.top().0 + top;
        let width = (border_box.size.width.0 - left - right).max(0.0);
        let height = (border_box.size.height.0 - top - bottom).max(0.0);
        let [tl, tr, br, bl] = radii;
        Self {
            bounds: [
                origin_left,
                origin_top,
                origin_left + width,
                origin_top + height,
            ],
            radii_x: [
                (tl - left).max(0.0),
                (tr - right).max(0.0),
                (br - right).max(0.0),
                (bl - left).max(0.0),
            ],
            radii_y: [
                (tl - top).max(0.0),
                (tr - top).max(0.0),
                (br - bottom).max(0.0),
                (bl - bottom).max(0.0),
            ],
        }
    }

    /// The same shape moved by `(dx, dy)` device pixels.
    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        let [left, top, right, bottom] = self.bounds;
        Self {
            bounds: [left + dx, top + dy, right + dx, bottom + dy],
            ..*self
        }
    }

    /// Coverage of this shape at fragment position `p` in device pixels (RND-002 R13).
    pub fn coverage(&self, p: [f32; 2]) -> f32 {
        let [left, top, right, bottom] = self.bounds;
        let extent = [right - left, bottom - top];
        if !(extent[0] > 0.0 && extent[1] > 0.0) {
            return 0.0;
        }
        let local = [p[0] - left, p[1] - top];
        let mut d = (-local[0])
            .max(-local[1])
            .max(local[0] - extent[0])
            .max(local[1] - extent[1]);
        // Top-left, top-right, bottom-right, bottom-left.
        for corner in 0..4 {
            let rx = self.radii_x[corner];
            let ry = self.radii_y[corner];
            if !(rx > 0.0 && ry > 0.0) {
                continue;
            }
            let right_side = corner == 1 || corner == 2;
            let bottom_side = corner == 2 || corner == 3;
            let inward = [
                if right_side {
                    extent[0] - local[0]
                } else {
                    local[0]
                },
                if bottom_side {
                    extent[1] - local[1]
                } else {
                    local[1]
                },
            ];
            if inward[0] < rx && inward[1] < ry {
                let q = [(inward[0] - rx) / rx, (inward[1] - ry) / ry];
                let length = (q[0] * q[0] + q[1] * q[1]).sqrt();
                let gradient =
                    ((q[0] / rx).powi(2) + (q[1] / ry).powi(2)).sqrt() / length.max(1e-6);
                d = d.max((length - 1.0) / gradient.max(1e-6));
            }
        }
        (0.5 - d).clamp(0.0, 1.0)
    }
}

/// Total curved coverage `C` of a set of shapes: the minimum of every shape's coverage, or
/// 1 with none (RND-002 R14).
pub fn curved_clip_coverage<'a>(
    shapes: impl IntoIterator<Item = &'a CurvedClipShape>,
    p: [f32; 2],
) -> f32 {
    shapes
        .into_iter()
        .fold(1.0f32, |coverage, shape| coverage.min(shape.coverage(p)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentMask, Quad, ScaledPixels, Scene, point, size};

    fn approx(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 1e-5,
            "expected {expected}, got {actual}"
        );
    }

    fn top_band(band: f32) -> ScopeParams {
        ScopeParams {
            fade_edges: [0.0, 0.0, 0.0, 0.0],
            fade_bands: [band, 0.0, 0.0, 0.0],
            ..Default::default()
        }
    }

    #[test]
    fn edge_coverage_landmarks_follow_squared_ramp() {
        let scope = top_band(100.0);
        for (position, expected) in [
            (0.0, 0.0),
            (25.0, 0.0625),
            (50.0, 0.25),
            (75.0, 0.5625),
            (100.0, 1.0),
            (250.0, 1.0),
        ] {
            approx(scope.edge_coverage([10.0, position]), expected);
        }
    }

    #[test]
    fn simultaneous_edges_combine_by_minimum() {
        let scope = ScopeParams {
            fade_edges: [0.0, 1000.0, 0.0, 1000.0],
            fade_bands: [100.0, 0.0, 100.0, 0.0],
            ..Default::default()
        };
        // Both normalized distances are 0.5: minimum squared is 0.25, not 0.0625.
        approx(scope.edge_coverage([50.0, 50.0]), 0.25);
    }

    #[test]
    fn disabled_edges_never_attenuate() {
        let scope = ScopeParams {
            fade_edges: [0.0, 0.0, 0.0, 0.0],
            fade_bands: [0.0, -5.0, 0.0, 0.0],
            ..Default::default()
        };
        assert!(!scope.fade_active());
        approx(scope.edge_coverage([0.0, 0.0]), 1.0);
    }

    #[test]
    fn rounded_coverage_is_a_one_pixel_linear_transition() {
        let scope = ScopeParams {
            clip_bounds: [0.0, 0.0, 100.0, 100.0],
            clip_radius: 10.0,
            ..Default::default()
        };
        approx(scope.rounded_coverage([50.0, 99.5]), 1.0);
        approx(scope.rounded_coverage([50.0, 100.0]), 0.5);
        approx(scope.rounded_coverage([50.0, 100.5]), 0.0);
        // Inactive scope: right not greater than left.
        approx(ScopeParams::default().rounded_coverage([-9.0, 4.0]), 1.0);
    }

    #[test]
    fn rounded_radius_larger_than_half_size_is_clamped() {
        let scope = ScopeParams {
            clip_bounds: [0.0, 0.0, 100.0, 60.0],
            clip_radius: 1000.0,
            ..Default::default()
        };
        let clamped = ScopeParams {
            clip_radius: 30.0,
            ..scope
        };
        for p in [[1.0, 1.0], [10.0, 30.0], [50.0, 0.5], [99.0, 59.0]] {
            approx(scope.rounded_coverage(p), clamped.rounded_coverage(p));
        }
    }

    #[test]
    fn scope_factor_is_edge_times_rounded() {
        let scope = ScopeParams {
            fade_edges: [0.0, 0.0, 0.0, 0.0],
            fade_bands: [100.0, 0.0, 0.0, 0.0],
            clip_bounds: [0.0, 0.0, 100.0, 100.0],
            clip_radius: 0.0,
            ..Default::default()
        };
        approx(scope.factor([50.0, 100.0]), 1.0 * 0.5);
        approx(scope.factor([50.0, 50.0]), 0.25);
    }

    #[test]
    fn effective_band_is_at_least_one_logical_pixel_and_disabled_edges_are_zero() {
        let mut fade = EdgeFade::new(Bounds::default(), Pixels(0.0));
        fade.top = true;
        fade.bottom = true;
        fade.band_bottom = Some(Pixels(-3.0));
        fade.left = true;
        fade.band_left = Some(Pixels(20.0));
        assert_eq!(fade.effective_bands(), [1.0, 1.0, 20.0, 0.0]);
        let mut scope = ScopeParams::default();
        fade.scaled(2.0, &mut scope);
        assert_eq!(scope.fade_bands, [2.0, 2.0, 40.0, 0.0]);
    }

    #[test]
    fn conservative_fade_of_a_box_touching_the_edge_is_transparent() {
        let mut fade = EdgeFade::new(
            Bounds {
                origin: point(Pixels(0.0), Pixels(0.0)),
                size: size(Pixels(100.0), Pixels(100.0)),
            },
            Pixels(40.0),
        );
        fade.top = true;
        let touching = Bounds {
            origin: point(Pixels(10.0), Pixels(0.0)),
            size: size(Pixels(20.0), Pixels(20.0)),
        };
        approx(fade.conservative_factor(touching), 0.0);
        let inside = Bounds {
            origin: point(Pixels(10.0), Pixels(20.0)),
            size: size(Pixels(20.0), Pixels(20.0)),
        };
        approx(fade.conservative_factor(inside), 0.25);
    }

    #[test]
    fn curved_shape_matches_exact_circular_distance_for_equal_radii() {
        let shape = CurvedClipShape {
            bounds: [0.0, 0.0, 64.0, 64.0],
            radii_x: [24.0; 4],
            radii_y: [24.0; 4],
        };
        // Straight edges.
        approx(shape.coverage([32.0, 0.5]), 1.0);
        approx(shape.coverage([32.0, 0.0]), 0.5);
        approx(shape.coverage([32.0, -0.5]), 0.0);
        // Just inside and outside the top-left arc, on the diagonal.
        let center = 24.0f32;
        for offset in [-1.0f32, -0.25, 0.25, 1.0] {
            let along = center - (24.0 + offset) * std::f32::consts::FRAC_1_SQRT_2;
            let expected = (0.5 - offset).clamp(0.0, 1.0);
            approx(shape.coverage([along, along]), expected);
        }
    }

    #[test]
    fn empty_shape_covers_nothing_and_shapes_combine_by_minimum() {
        let empty = CurvedClipShape::default();
        approx(empty.coverage([0.0, 0.0]), 0.0);
        let a = CurvedClipShape {
            bounds: [0.0, 0.0, 64.0, 64.0],
            ..Default::default()
        };
        let b = CurvedClipShape {
            bounds: [32.0, 0.0, 96.0, 64.0],
            ..Default::default()
        };
        approx(curved_clip_coverage([&a, &b], [32.0, 32.0]), 0.5);
        approx(
            curved_clip_coverage([&a, &b], [16.0, 32.0]),
            0.0f32.max(b.coverage([16.0, 32.0])),
        );
        approx(curved_clip_coverage([], [1.0, 1.0]), 1.0);
    }

    #[test]
    fn padding_edge_shape_insets_by_borders_and_uses_elliptical_inner_radii() {
        let border_box = Bounds {
            origin: point(ScaledPixels(80.0), ScaledPixels(80.0)),
            size: size(ScaledPixels(840.0), ScaledPixels(560.0)),
        };
        // FID-RND-002-4 outer block at scale 2.
        let shape =
            CurvedClipShape::from_padding_edge(border_box, [80.0; 4], [8.0, 24.0, 40.0, 16.0]);
        assert_eq!(shape.bounds, [96.0, 88.0, 896.0, 600.0]);
        assert_eq!(shape.radii_x, [64.0, 56.0, 56.0, 64.0]);
        assert_eq!(shape.radii_y, [72.0, 72.0, 40.0, 40.0]);
    }

    fn quad_at(x: f32, width: f32) -> Quad {
        let bounds = Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(0.0)),
            size: size(ScaledPixels(width), ScaledPixels(10.0)),
        };
        Quad {
            bounds,
            content_mask: ContentMask { bounds },
            ..Default::default()
        }
    }

    fn shape(left: f32) -> CurvedClipShape {
        CurvedClipShape {
            bounds: [left, 0.0, left + 50.0, 50.0],
            ..Default::default()
        }
    }

    #[test]
    fn scene_allocates_no_curved_clip_data_without_opted_in_primitives() {
        let mut scene = Scene::default();
        scene.insert_primitive(quad_at(0.0, 10.0));
        scene.push_curved_clip(shape(0.0));
        scene.push_curved_clip(shape(5.0));
        scene.pop_curved_clip();
        scene.pop_curved_clip();
        assert!(scene.curved_clips.is_empty());
        assert_eq!(scene.quads[0].scope.curved_clip, 0);
    }

    #[test]
    fn nested_shapes_form_a_chain_in_paint_order() {
        let mut scene = Scene::default();
        scene.push_curved_clip(shape(0.0));
        scene.insert_primitive(quad_at(0.0, 10.0));
        scene.push_curved_clip(shape(5.0));
        scene.push_curved_clip(shape(7.0));
        scene.insert_primitive(quad_at(20.0, 10.0));
        scene.pop_curved_clip();
        scene.insert_primitive(quad_at(40.0, 10.0));
        scene.pop_curved_clip();
        scene.insert_primitive(quad_at(60.0, 10.0));
        scene.pop_curved_clip();
        scene.insert_primitive(quad_at(80.0, 10.0));
        let chains: Vec<u32> = scene.quads.iter().map(|q| q.scope.curved_clip).collect();
        // Nodes: 1 = outer, 2 = middle, 3 = innermost.
        assert_eq!(chains, vec![1, 3, 2, 1, 0]);
        assert_eq!(scene.curved_clip_parents, vec![0, 1, 2]);
        assert_eq!(scene.curved_clips[2], shape(7.0));
    }

    #[test]
    fn replay_places_own_shapes_under_the_destination_chain_and_translates_them() {
        let mut previous = Scene::default();
        previous.push_curved_clip(shape(0.0)); // ancestor: not part of the replayed range
        let start = previous.len();
        previous.push_curved_clip(shape(10.0));
        previous.insert_primitive(quad_at(0.0, 10.0));
        previous.pop_curved_clip();
        previous.insert_primitive(quad_at(20.0, 10.0));
        let end = previous.len();
        previous.pop_curved_clip();

        let mut next = Scene::default();
        next.push_curved_clip(shape(100.0)); // different destination ancestor
        next.replay_translated(
            start..end,
            &previous,
            point(ScaledPixels(3.0), ScaledPixels(4.0)),
        );
        next.pop_curved_clip();

        // Destination ancestor = node 1; the replayed own shape is node 2, translated.
        assert_eq!(next.curved_clips[0], shape(100.0));
        assert_eq!(next.curved_clips[1], shape(10.0).translated(3.0, 4.0));
        assert_eq!(next.curved_clip_parents, vec![0, 1]);
        let chains: Vec<u32> = next.quads.iter().map(|q| q.scope.curved_clip).collect();
        assert_eq!(chains, vec![2, 1]);
    }

    #[test]
    fn replay_of_a_balanced_range_matches_direct_painting() {
        let paint = |scene: &mut Scene| {
            scene.push_curved_clip(shape(0.0));
            scene.insert_primitive(quad_at(0.0, 10.0));
            scene.push_curved_clip(shape(5.0));
            scene.insert_primitive(quad_at(20.0, 10.0));
            scene.pop_curved_clip();
            scene.pop_curved_clip();
        };
        let mut direct = Scene::default();
        paint(&mut direct);
        let mut replayed = Scene::default();
        replayed.replay(0..direct.len(), &direct);
        assert_eq!(replayed.curved_clips, direct.curved_clips);
        assert_eq!(replayed.curved_clip_parents, direct.curved_clip_parents);
        let chains = |scene: &Scene| {
            scene
                .quads
                .iter()
                .map(|q| q.scope.curved_clip)
                .collect::<Vec<_>>()
        };
        assert_eq!(chains(&replayed), chains(&direct));
    }

    #[test]
    fn deep_nesting_is_not_limited() {
        let mut scene = Scene::default();
        for depth in 0..500 {
            scene.push_curved_clip(shape(depth as f32));
        }
        scene.insert_primitive(quad_at(0.0, 10.0));
        for _ in 0..500 {
            scene.pop_curved_clip();
        }
        assert_eq!(scene.curved_clips.len(), 500);
        assert_eq!(scene.quads[0].scope.curved_clip, 500);
        assert_eq!(scene.curved_clip_parents[499], 499);
    }
}

// GPU record sizes: Direct3D constant buffers need 16-byte multiples and the shaders index
// arrays of these records, so the sizes are part of the renderer contract.
const _: () = {
    assert!(std::mem::size_of::<ScopeParams>() == 64);
    assert!(std::mem::size_of::<CurvedClipShape>() == 48);
    assert!(std::mem::size_of::<crate::ImageEffects>() == 48);
    assert!(std::mem::size_of::<crate::Quad>().is_multiple_of(16));
    assert!(std::mem::size_of::<crate::MonochromeSprite>().is_multiple_of(16));
    assert!(std::mem::size_of::<crate::SubpixelSprite>().is_multiple_of(16));
    assert!(std::mem::size_of::<crate::PolychromeSprite>().is_multiple_of(16));
};
