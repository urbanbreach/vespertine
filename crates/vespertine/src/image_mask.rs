//! Feathered inverse rounded-rectangle image masks (RND-004) and the nearest-texel selection
//! of raw-pixel painting (RND-007): the window-level descriptor, its normalization and
//! device-pixel conversion, and the reference arithmetic the GPU shader evaluates per fragment.
//!
//! The Metal and Direct3D renderers carry the [`ImageEffects`] block but do not evaluate it:
//! on those backends images are painted without the mask and with linear filtering.

use crate::{Bounds, ImageEffects, Pixels, px};

/// A rounded-rectangle exclusion that attenuates an image around a window-space region
/// (RND-004).
///
/// Everything is in window-space logical pixels; [`ImageAlphaMask::to_effects`] normalizes the
/// descriptor and converts it to device pixels. The region *removes* the image at its center
/// and reveals the image around it, feathering outward with a cubic ramp, unlike the
/// scoped rounded clip that keeps its inside.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ImageAlphaMask {
    /// The excluded rectangle in window space.
    pub bounds: Bounds<Pixels>,
    /// Uniform corner radius of the excluded rectangle; clamped to half its shorter side.
    pub radius: Pixels,
    /// Width of the outward feather; zero or negative disables the whole mask.
    pub feather: Pixels,
    /// Extra distance by which the cutout is enlarged before the feather starts.
    pub clearance: Pixels,
    /// Optional bottom fade: the window-space y at which the image is fully hidden, and the
    /// height of the band above it over which the image fades in.
    pub bottom_fade: Option<(Pixels, Pixels)>,
}

impl ImageAlphaMask {
    /// Returns the descriptor normalized as RND-004 R10 requires: radius clamped to
    /// `[0, half of the shorter nonnegative bound dimension]`, and feather, clearance and
    /// bottom band height clamped to nonnegative values.
    pub fn normalized(&self) -> Self {
        let half_shorter = self
            .bounds
            .size
            .width
            .0
            .max(0.0)
            .min(self.bounds.size.height.0.max(0.0))
            / 2.0;
        Self {
            bounds: self.bounds,
            radius: px(self.radius.0.clamp(0.0, half_shorter)),
            feather: px(self.feather.0.max(0.0)),
            clearance: px(self.clearance.0.max(0.0)),
            bottom_fade: self
                .bottom_fade
                .map(|(edge, band)| (edge, px(band.0.max(0.0)))),
        }
    }

    /// Returns the per-draw image effects for a window with `scale_factor`: the normalized
    /// descriptor with every geometric quantity multiplied by the scale factor (not snapped
    /// to device pixels).
    ///
    /// A zero effective feather yields the neutral block, which paints exactly like an image
    /// without a mask, including a disabled bottom fade.
    pub fn to_effects(&self, scale_factor: f32) -> ImageEffects {
        let mask = self.normalized();
        if mask.feather.0 * scale_factor <= 0.0 {
            return ImageEffects::default();
        }
        let (bottom_reference, bottom_feather) = match mask.bottom_fade {
            Some((edge, band)) => (edge.0 * scale_factor, band.0 * scale_factor),
            None => (0.0, 0.0),
        };
        ImageEffects {
            mask_bounds: [
                mask.bounds.origin.x.0 * scale_factor,
                mask.bounds.origin.y.0 * scale_factor,
                mask.bounds.size.width.0 * scale_factor,
                mask.bounds.size.height.0 * scale_factor,
            ],
            mask_radius: mask.radius.0 * scale_factor,
            mask_feather: mask.feather.0 * scale_factor,
            mask_clearance: mask.clearance.0 * scale_factor,
            mask_bottom_reference: bottom_reference,
            mask_bottom_feather: bottom_feather,
            ..ImageEffects::default()
        }
    }
}

/// Cubic Hermite step `v^2 (3 - 2 v)` with `v = clamp((z - a) / (b - a), 0, 1)`; `b > a`.
fn smoothstep(a: f32, b: f32, z: f32) -> f32 {
    let v = ((z - a) / (b - a)).clamp(0.0, 1.0);
    v * v * (3.0 - 2.0 * v)
}

impl ImageEffects {
    /// Signed distance from `point` (device pixels) to the mask rectangle, using the
    /// radius as supplied (no half-size clamp; see RND-004 R3).
    pub fn mask_distance(&self, point: [f32; 2]) -> f32 {
        let [x, y, width, height] = self.mask_bounds;
        let half = [width / 2.0, height / 2.0];
        let center = [x + half[0], y + half[1]];
        let radius = self.mask_radius;
        let e = [
            (point[0] - center[0]).abs() - half[0] + radius,
            (point[1] - center[1]).abs() - half[1] + radius,
        ];
        let outside = (e[0].max(0.0).powi(2) + e[1].max(0.0).powi(2)).sqrt();
        outside + e[0].max(e[1]).min(0.0) - radius
    }

    /// Mask coverage in `[0, 1]` at `point` (device pixels, the fragment center): the
    /// reference arithmetic of the shader (RND-004 R2-R5).
    ///
    /// A feather that is zero or negative disables the whole mask (coverage 1). Otherwise the
    /// base coverage is `smoothstep(0, feather, distance - clearance)`; a positive bottom
    /// feather additionally takes the minimum with `smoothstep(0, bottom feather, bottom
    /// reference - y)`.
    pub fn mask_coverage(&self, point: [f32; 2]) -> f32 {
        if self.mask_feather <= 0.0 {
            return 1.0;
        }
        let mut coverage = smoothstep(
            0.0,
            self.mask_feather,
            self.mask_distance(point) - self.mask_clearance,
        );
        if self.mask_bottom_feather > 0.0 {
            coverage = coverage.min(smoothstep(
                0.0,
                self.mask_bottom_feather,
                self.mask_bottom_reference - point[1],
            ));
        }
        coverage
    }

    /// Whether the nearest-neighbor texel selection flag is set.
    pub fn is_nearest(&self) -> bool {
        self.flags & Self::NEAREST != 0
    }
}

/// Selects the texel of an atlas axis that contains a normalized coordinate (RND-007 R4):
/// the coordinate times the atlas dimension is truncated toward zero and capped at the final
/// texel (`dimension - 1`) so a coordinate exactly on the upper boundary stays inside the
/// atlas. A negative coordinate selects texel 0. The shader mirrors this per axis.
pub fn nearest_texel(coordinate: f32, dimension: u32) -> u32 {
    let last = dimension.saturating_sub(1);
    let texel = (coordinate * dimension as f32).trunc();
    if texel <= 0.0 {
        0
    } else {
        (texel as u32).min(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{point, size};

    fn effects(feather: f32, clearance: f32) -> ImageEffects {
        ImageEffects {
            mask_bounds: [0.0, 0.0, 200.0, 100.0],
            mask_radius: 0.0,
            mask_feather: feather,
            mask_clearance: clearance,
            ..Default::default()
        }
    }

    /// A point to the right of the rectangle at signed distance `distance`.
    fn at_distance(distance: f32) -> [f32; 2] {
        [200.0 + distance, 50.0]
    }

    #[test]
    fn feather_ramp_is_a_cubic_hermite_from_the_cutout_edge() {
        // RND-004 A1: clearance 0, feather 20.
        let mask = effects(20.0, 0.0);
        assert_eq!(mask.mask_coverage(at_distance(0.0)), 0.0);
        assert_eq!(mask.mask_coverage(at_distance(10.0)), 0.5);
        assert_eq!(mask.mask_coverage(at_distance(20.0)), 1.0);
        assert_eq!(mask.mask_coverage(at_distance(5.0)), 0.15625);
        assert_eq!(mask.mask_coverage(at_distance(-5.0)), 0.0);
        assert_eq!(mask.mask_coverage(at_distance(400.0)), 1.0);
        // The cutout center is removed entirely.
        assert_eq!(mask.mask_coverage([100.0, 50.0]), 0.0);
    }

    #[test]
    fn clearance_shifts_the_ramp_outward_and_negative_clearance_inward() {
        // RND-004 A2: clearance 5, feather 20 moves the landmarks to 5, 15 and 25.
        let mask = effects(20.0, 5.0);
        assert_eq!(mask.mask_coverage(at_distance(5.0)), 0.0);
        assert_eq!(mask.mask_coverage(at_distance(15.0)), 0.5);
        assert_eq!(mask.mask_coverage(at_distance(25.0)), 1.0);
        // The evaluation level supports negative clearance (the descriptor clamps it).
        let inward = effects(20.0, -5.0);
        assert_eq!(inward.mask_coverage(at_distance(-5.0)), 0.0);
        assert_eq!(inward.mask_coverage(at_distance(5.0)), 0.5);
        assert_eq!(inward.mask_coverage(at_distance(15.0)), 1.0);
    }

    #[test]
    fn rounded_corners_use_the_radius_without_clamping() {
        let mut mask = effects(10.0, 0.0);
        mask.mask_radius = 30.0;
        // Corner distance: the rounded corner circle is centered 30 inside each edge.
        // The point on the corner arc's 45-degree diagonal at distance d from the arc.
        let center = [200.0 - 30.0, 100.0 - 30.0];
        let diagonal = std::f32::consts::FRAC_1_SQRT_2;
        let on_arc = [center[0] + 30.0 * diagonal, center[1] + 30.0 * diagonal];
        assert!(mask.mask_distance(on_arc).abs() < 1e-4);
        let outside = [
            center[0] + (30.0 + 5.0) * diagonal,
            center[1] + (30.0 + 5.0) * diagonal,
        ];
        assert!((mask.mask_distance(outside) - 5.0).abs() < 1e-4);
        assert!((mask.mask_coverage(outside) - 0.5).abs() < 1e-5);
        // The evaluation does not clamp an oversized radius: on this 100 high rectangle a
        // radius of 80 gives a different shape than the clamped radius of 50.
        let point = [190.0, 50.0];
        mask.mask_radius = 50.0;
        assert!((mask.mask_distance(point) - (-10.0)).abs() < 1e-4);
        mask.mask_radius = 80.0;
        assert!((mask.mask_distance(point) - (5800.0f32.sqrt() - 80.0)).abs() < 1e-4);
    }

    #[test]
    fn bottom_ramp_combines_by_minimum_not_product() {
        // RND-004 A3: bottom reference 440 logical, bottom feather 36 logical, scale 2.
        let scale = 2.0;
        let mask = ImageEffects {
            mask_bounds: [0.0, 0.0, 10.0, 10.0],
            mask_radius: 0.0,
            mask_feather: 24.0 * scale,
            mask_clearance: 0.0,
            mask_bottom_reference: 440.0 * scale,
            mask_bottom_feather: 36.0 * scale,
            ..Default::default()
        };
        // Far from the cutout: the base coverage is 1, the bottom ramp alone shows.
        let x = 300.0 * scale;
        assert_eq!(mask.mask_coverage([x, 404.0 * scale]), 1.0);
        assert_eq!(mask.mask_coverage([x, 380.0 * scale]), 1.0);
        assert_eq!(mask.mask_coverage([x, 422.0 * scale]), 0.5);
        assert_eq!(mask.mask_coverage([x, 440.0 * scale]), 0.0);
        assert_eq!(mask.mask_coverage([x, 450.0 * scale]), 0.0);

        // Both ramps at 0.5: coverage 0.5, not 0.25.
        let both = ImageEffects {
            mask_bounds: [0.0, 0.0, 200.0, 100.0],
            mask_feather: 20.0,
            mask_bottom_reference: 50.0 + 10.0,
            mask_bottom_feather: 20.0,
            ..Default::default()
        };
        // Right of the rectangle at distance 10 on the row y = 50: base 0.5; bottom 10 above
        // the reference with band 20: 0.5.
        assert_eq!(both.mask_coverage([210.0, 50.0]), 0.5);
    }

    #[test]
    fn non_positive_feather_disables_the_whole_mask() {
        let mut mask = effects(0.0, 4.0);
        mask.mask_bottom_reference = 100.0;
        mask.mask_bottom_feather = 50.0;
        assert_eq!(mask.mask_coverage([100.0, 50.0]), 1.0);
        assert_eq!(mask.mask_coverage([100.0, 500.0]), 1.0);
        mask.mask_feather = -3.0;
        assert_eq!(mask.mask_coverage([100.0, 50.0]), 1.0);
        // A non-positive bottom feather disables only the bottom ramp.
        mask.mask_feather = 10.0;
        mask.mask_bottom_feather = 0.0;
        assert_eq!(mask.mask_coverage([100.0, 500.0]), 1.0);
        assert_eq!(mask.mask_coverage([100.0, 50.0]), 0.0);
        mask.mask_bottom_feather = -2.0;
        assert_eq!(mask.mask_coverage([100.0, 500.0]), 1.0);
    }

    fn descriptor() -> ImageAlphaMask {
        ImageAlphaMask {
            bounds: Bounds {
                origin: point(px(180.0), px(110.0)),
                size: size(px(520.0), px(230.0)),
            },
            radius: px(36.0),
            feather: px(24.0),
            clearance: px(8.0),
            bottom_fade: Some((px(440.0), px(36.0))),
        }
    }

    #[test]
    fn descriptor_scales_every_quantity_by_the_scale_factor() {
        // FID-RND-004-1 mask at scale 2.
        let effects = descriptor().to_effects(2.0);
        assert_eq!(effects.mask_bounds, [360.0, 220.0, 1040.0, 460.0]);
        assert_eq!(effects.mask_radius, 72.0);
        assert_eq!(effects.mask_feather, 48.0);
        assert_eq!(effects.mask_clearance, 16.0);
        assert_eq!(effects.mask_bottom_reference, 880.0);
        assert_eq!(effects.mask_bottom_feather, 72.0);
        assert_eq!(effects.flags, 0);
        // Not snapped to device pixels.
        let mut fractional = descriptor();
        fractional.bounds.origin = point(px(0.3), px(0.7));
        let effects = fractional.to_effects(1.5);
        assert!((effects.mask_bounds[0] - 0.45).abs() < 1e-6);
        assert!((effects.mask_bounds[1] - 1.05).abs() < 1e-6);
    }

    #[test]
    fn radius_is_clamped_before_scaling() {
        let mut mask = descriptor();
        mask.bounds.size = size(px(100.0), px(40.0));
        mask.radius = px(500.0);
        assert_eq!(mask.normalized().radius, px(20.0));
        assert_eq!(mask.to_effects(2.0).mask_radius, 40.0);
        mask.radius = px(-5.0);
        assert_eq!(mask.to_effects(2.0).mask_radius, 0.0);
        // A negative dimension clamps like zero.
        mask.radius = px(10.0);
        mask.bounds.size = size(px(-30.0), px(40.0));
        assert_eq!(mask.normalized().radius, px(0.0));
    }

    #[test]
    fn negative_feather_clearance_and_band_clamp_to_zero() {
        let mut mask = descriptor();
        mask.clearance = px(-8.0);
        mask.bottom_fade = Some((px(440.0), px(-36.0)));
        let normalized = mask.normalized();
        assert_eq!(normalized.clearance, px(0.0));
        assert_eq!(normalized.bottom_fade, Some((px(440.0), px(0.0))));
        let effects = mask.to_effects(2.0);
        assert_eq!(effects.mask_clearance, 0.0);
        assert_eq!(effects.mask_bottom_feather, 0.0);
        assert_eq!(effects.mask_bottom_reference, 880.0);
        // The disabled band leaves only the base mask.
        assert_eq!(effects.mask_coverage([1000.0, 900.0]), 1.0);

        mask.feather = px(-4.0);
        assert_eq!(mask.normalized().feather, px(0.0));
        assert_eq!(mask.to_effects(2.0), ImageEffects::default());
    }

    #[test]
    fn absent_bottom_fade_means_bottom_feather_zero() {
        let mut mask = descriptor();
        mask.bottom_fade = None;
        let effects = mask.to_effects(2.0);
        assert_eq!(effects.mask_bottom_feather, 0.0);
        assert_eq!(effects.mask_bottom_reference, 0.0);
    }

    #[test]
    fn zero_feather_yields_the_neutral_block_even_with_a_bottom_fade() {
        let mut mask = descriptor();
        mask.feather = px(0.0);
        let effects = mask.to_effects(2.0);
        assert_eq!(effects, ImageEffects::default());
        assert_eq!(effects.mask_coverage([600.0, 400.0]), 1.0);
        assert_eq!(effects.mask_coverage([600.0, 1800.0]), 1.0);
    }

    #[test]
    fn nearest_texel_truncates_and_caps_at_the_last_texel() {
        // RND-007 A4: dimension 8.
        assert_eq!(nearest_texel(0.0, 8), 0);
        // Just below a texel boundary selects the lower texel.
        assert_eq!(nearest_texel(0.375 - 1e-4, 8), 2);
        assert_eq!(nearest_texel(0.375, 8), 3);
        assert_eq!(nearest_texel(0.375 + 1e-4, 8), 3);
        assert_eq!(nearest_texel(0.999, 8), 7);
        // Exactly on the upper atlas boundary: the last texel, not one past it.
        assert_eq!(nearest_texel(1.0, 8), 7);
        assert_eq!(nearest_texel(1.0, 1024), 1023);
        assert_eq!(nearest_texel(1.5, 1024), 1023);
        // A non-power-of-two atlas dimension.
        assert_eq!(nearest_texel(1.0, 1000), 999);
        assert_eq!(nearest_texel(0.9995 - 1e-6, 1000), 999);
        assert_eq!(nearest_texel(0.5, 1000), 500);
        assert_eq!(nearest_texel(0.4999, 1000), 499);
        // Truncation toward zero, never rounding.
        assert_eq!(nearest_texel(0.7499, 4), 2);
        assert_eq!(nearest_texel(-0.2, 4), 0);
    }

    #[test]
    fn nearest_flag_is_reported() {
        let mut effects = ImageEffects::default();
        assert!(!effects.is_nearest());
        effects.flags |= ImageEffects::NEAREST;
        assert!(effects.is_nearest());
    }
}
