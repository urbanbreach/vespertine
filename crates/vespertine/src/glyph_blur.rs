//! Reference arithmetic of the composite-time glyph blur (RND-005).
//!
//! The GPU evaluates a Gaussian neighborhood while sampling the cached monochrome glyph
//! tile (see `Window::paint_glyph_transformed`); the functions here state the same math in
//! plain Rust so that the geometry, the tile mapping and the kernel are checked against the
//! pixels the renderer produces.
//!
//! Units: the blur sigma of a [`MonochromeSprite`](crate::MonochromeSprite) is `sigma_d`, in
//! device pixels. It is at once the geometry padding (the sprite is inflated by
//! `3 * sigma_d` on every side) and the Gaussian sigma measured in atlas texels, so the two
//! agree only when a tile texel is one device pixel (RND-005 R8).

use crate::{Bounds, Pixels, ScaledPixels};

/// Whether the renderer of this target evaluates the glyph blur kernel. The wgpu renderer
/// (Linux and other non-Apple, non-Windows targets) does; the Metal and Direct3D shaders
/// keep the ordinary tile mapping, so there `Window::paint_glyph_transformed` treats the
/// blur as zero and paints the transformed glyph unblurred.
pub const GLYPH_BLUR_SUPPORTED: bool = cfg!(not(any(target_os = "macos", target_os = "windows")));

/// Halo of the destination rectangle, in multiples of the device sigma.
pub const BLUR_HALO_SIGMAS: f32 = 3.0;

/// Largest tap radius of the discrete kernel: at most 25 by 25 = 625 taps.
pub const MAX_BLUR_TAP_RADIUS: u32 = 12;

#[cfg(any(test, feature = "test-support"))]
/// Floor of the unpadded size in the tile mapping and of the accumulation denominator.
pub const BLUR_EPSILON: f32 = 0.000001;

#[cfg(any(test, feature = "test-support"))]
/// Distance, in texels, a tap is kept away from the tile edge before it is sampled.
pub const BLUR_TILE_INSET_TEXELS: f32 = 0.5;

/// Device-pixel sigma for a logical blur: scaled by the window scale factor and clamped to
/// zero from below (negative and NaN blurs are no blur).
pub fn device_blur_sigma(logical_blur: Pixels, scale_factor: f32) -> f32 {
    (logical_blur.0 * scale_factor).max(0.0)
}

/// Halo added on every side of the destination rectangle for `sigma_d`: `3 * sigma_d`.
pub fn blur_halo(sigma_d: f32) -> f32 {
    BLUR_HALO_SIGMAS * sigma_d.max(0.0)
}

/// Tap radius of the Gaussian neighborhood: `min(ceil(3 * sigma_d), 12)`, `0` without blur.
/// The halo keeps growing with sigma; only the number of taps is capped (RND-005 R9).
pub fn blur_kernel_radius(sigma_d: f32) -> u32 {
    if sigma_d > 0.0 {
        ((BLUR_HALO_SIGMAS * sigma_d)
            .ceil()
            .min(MAX_BLUR_TAP_RADIUS as f32)) as u32
    } else {
        0
    }
}

/// The destination rectangle of a glyph tile expanded by the halo of `sigma_d` on every
/// side; the identity for `sigma_d <= 0`.
pub fn blur_expanded_bounds(bounds: Bounds<ScaledPixels>, sigma_d: f32) -> Bounds<ScaledPixels> {
    let halo = blur_halo(sigma_d);
    if halo > 0.0 {
        bounds.dilate(ScaledPixels(halo))
    } else {
        bounds
    }
}

#[cfg(any(test, feature = "test-support"))]
/// Source tile coordinate, in texels, of the normalized coordinate `unit` (0 to 1) across
/// the expanded destination size `expanded_size` (device pixels) for a tile of `tile_size`
/// texels and halo `halo`: `(unit * expanded_size - halo) * tile_size /
/// max(expanded_size - 2 * halo, 0.000001)`. One axis; with zero halo this reduces to
/// `unit * tile_size`.
pub fn blur_tile_coordinate(unit: f32, expanded_size: f32, halo: f32, tile_size: f32) -> f32 {
    (unit * expanded_size - halo) * tile_size / (expanded_size - 2.0 * halo).max(BLUR_EPSILON)
}

#[cfg(any(test, feature = "test-support"))]
/// Weight of the tap at integer offset `(x, y)` texels: `exp(-(x^2 + y^2) / (2 sigma^2))`.
pub fn blur_tap_weight(x: i32, y: i32, sigma: f32) -> f32 {
    (-((x * x + y * y) as f32) / (2.0 * sigma * sigma)).exp()
}

#[cfg(any(test, feature = "test-support"))]
/// Sum of the weights of the whole `(2r + 1)^2` neighborhood. This is the divisor of the
/// blurred coverage, including taps that fall outside the tile.
pub fn blur_weight_sum(sigma: f32) -> f32 {
    let radius = blur_kernel_radius(sigma) as i32;
    let mut sum = 0.0;
    for y in -radius..=radius {
        for x in -radius..=radius {
            sum += blur_tap_weight(x, y, sigma);
        }
    }
    sum
}

#[cfg(any(test, feature = "test-support"))]
/// Blurred coverage at `center`, a position in texels measured from the top-left corner of
/// a tile of `tile_size` texels, over the tile whose coverage texel `(x, y)` is
/// `texel(x, y)` (0 to 1).
///
/// Every neighborhood offset `(x, y)`, `-r <= x, y <= r`, is one tap of weight
/// [`blur_tap_weight`]. A tap outside the inclusive tile rectangle is transparent; one
/// inside is clamped to the tile inset by half a texel (collapsing toward the midpoint of a
/// tile narrower than one texel) and sampled bilinearly. The weighted sum is divided by
/// `max(blur_weight_sum, 0.000001)`, so edges fade toward transparent instead of
/// brightening. With `sigma_d <= 0` the single bilinear sample at `center` is returned
/// without a halo, as the ordinary path does.
pub fn blurred_coverage(
    sigma_d: f32,
    center: [f32; 2],
    tile_size: [u32; 2],
    texel: impl Fn(u32, u32) -> f32,
) -> f32 {
    let size = [tile_size[0] as f32, tile_size[1] as f32];
    let bilinear = |p: [f32; 2]| {
        let mut clamped = [0.0; 2];
        for axis in 0..2 {
            let mut lo = BLUR_TILE_INSET_TEXELS;
            let mut hi = size[axis] - BLUR_TILE_INSET_TEXELS;
            if lo > hi {
                lo = size[axis] * 0.5;
                hi = lo;
            }
            clamped[axis] = p[axis].clamp(lo, hi);
        }
        let u = [clamped[0] - 0.5, clamped[1] - 0.5];
        let base = [u[0].floor(), u[1].floor()];
        let frac = [u[0] - base[0], u[1] - base[1]];
        let fetch = |dx: f32, dy: f32| {
            let x = (base[0] + dx).clamp(0.0, size[0] - 1.0) as u32;
            let y = (base[1] + dy).clamp(0.0, size[1] - 1.0) as u32;
            texel(x, y)
        };
        let top = fetch(0.0, 0.0) * (1.0 - frac[0]) + fetch(1.0, 0.0) * frac[0];
        let bottom = fetch(0.0, 1.0) * (1.0 - frac[0]) + fetch(1.0, 1.0) * frac[0];
        top * (1.0 - frac[1]) + bottom * frac[1]
    };
    if sigma_d <= 0.0 {
        return bilinear(center);
    }
    let radius = blur_kernel_radius(sigma_d) as i32;
    let mut sum = 0.0;
    let mut total = 0.0;
    for y in -radius..=radius {
        for x in -radius..=radius {
            let weight = blur_tap_weight(x, y, sigma_d);
            total += weight;
            let p = [center[0] + x as f32, center[1] + y as f32];
            let inside = p[0] >= 0.0 && p[0] <= size[0] && p[1] >= 0.0 && p[1] <= size[1];
            if inside {
                sum += weight * bilinear(p);
            }
        }
    }
    sum / total.max(BLUR_EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Point, Size, px};

    #[test]
    fn halo_and_radius_follow_the_device_sigma() {
        // A2: logical sigma 1 at scale 2 is device sigma 2: halo 6, radius 6.
        let sigma = device_blur_sigma(px(1.0), 2.0);
        assert_eq!(sigma, 2.0);
        assert_eq!(blur_halo(sigma), 6.0);
        assert_eq!(blur_kernel_radius(sigma), 6);
        // A6: the radius is capped at 12 while the halo keeps growing.
        assert_eq!((blur_halo(8.0), blur_kernel_radius(8.0)), (24.0, 12));
        assert_eq!((blur_halo(5.0), blur_kernel_radius(5.0)), (15.0, 12));
        assert_eq!((blur_halo(4.0), blur_kernel_radius(4.0)), (12.0, 12));
        assert_eq!(blur_kernel_radius(3.9), 12);
        assert_eq!(blur_kernel_radius(0.5), 2);
        assert_eq!(blur_kernel_radius(100.0), 12);
        assert_eq!(blur_halo(100.0), 300.0);
    }

    #[test]
    fn negative_and_zero_blur_mean_no_blur() {
        assert_eq!(device_blur_sigma(px(-3.0), 2.0), 0.0);
        assert_eq!(device_blur_sigma(px(f32::NAN), 2.0), 0.0);
        assert_eq!(device_blur_sigma(px(0.0), 2.0), 0.0);
        assert_eq!(blur_halo(-1.0), 0.0);
        assert_eq!(blur_kernel_radius(0.0), 0);
        assert_eq!(blur_kernel_radius(-2.0), 0);
        let bounds = Bounds {
            origin: Point::new(ScaledPixels(3.0), ScaledPixels(4.0)),
            size: Size {
                width: ScaledPixels(10.0),
                height: ScaledPixels(12.0),
            },
        };
        assert_eq!(blur_expanded_bounds(bounds, 0.0), bounds);
        assert_eq!(blur_expanded_bounds(bounds, -4.0), bounds);
    }

    #[test]
    fn expanded_bounds_grow_by_three_sigma_per_side() {
        let bounds = Bounds {
            origin: Point::new(ScaledPixels(20.0), ScaledPixels(30.0)),
            size: Size {
                width: ScaledPixels(10.0),
                height: ScaledPixels(12.0),
            },
        };
        let expanded = blur_expanded_bounds(bounds, 2.0);
        assert_eq!(expanded.origin.x.0, 14.0);
        assert_eq!(expanded.origin.y.0, 24.0);
        assert_eq!(expanded.size.width.0, 22.0);
        assert_eq!(expanded.size.height.0, 24.0);
    }

    #[test]
    fn tile_mapping_keeps_content_size_and_alignment() {
        // The original content (device px h..Q-h) maps to the whole tile whatever the
        // oversampling of the tile, so expanding the quad never scales the content.
        let (original, tile) = (10.0_f32, 20.0_f32);
        for sigma in [0.5_f32, 2.0, 5.0, 8.0] {
            let halo = blur_halo(sigma);
            let q = original + 2.0 * halo;
            assert!(blur_tile_coordinate(halo / q, q, halo, tile).abs() < 1e-4);
            assert!((blur_tile_coordinate(1.0 - halo / q, q, halo, tile) - tile).abs() < 1e-3);
            // Halo corners lie `halo` device pixels outside the tile.
            let outside = blur_tile_coordinate(0.0, q, halo, tile);
            assert!((outside + halo * tile / original).abs() < 1e-3);
        }
        // Zero halo reduces to the ordinary mapping.
        assert_eq!(blur_tile_coordinate(0.25, 10.0, 0.0, 20.0), 5.0);
        // A collapsed unpadded size is floored, not divided by zero.
        assert!(blur_tile_coordinate(0.5, 6.0, 3.0, 4.0).is_finite());
    }

    #[test]
    fn weights_are_gaussian_and_normalized_over_the_whole_neighborhood() {
        assert_eq!(blur_tap_weight(0, 0, 2.0), 1.0);
        let expected = (-(3.0_f32 * 3.0 + 4.0 * 4.0) / (2.0 * 2.0 * 2.0)).exp();
        assert!((blur_tap_weight(3, 4, 2.0) - expected).abs() < 1e-6);
        assert_eq!(blur_tap_weight(2, 1, 2.0), blur_tap_weight(-1, -2, 2.0));
        let sigma = 1.5;
        let r = blur_kernel_radius(sigma) as i32;
        assert_eq!(r, 5);
        let sum = blur_weight_sum(sigma);
        // The neighborhood is (2r+1)^2 taps, and for r >= 3 sigma contains almost all of
        // the continuous Gaussian mass 2 pi sigma^2.
        let continuous = 2.0 * std::f32::consts::PI * sigma * sigma;
        assert!((sum - continuous).abs() / continuous < 0.02);
    }

    #[test]
    fn edge_coverage_fades_instead_of_renormalizing_over_in_tile_taps() {
        // A solid 8x8 tile: the center of the tile keeps coverage 1; a point on the border
        // gets about half, and a corner about a quarter, of the full coverage.
        let solid = |_: u32, _: u32| 1.0;
        let sigma = 2.0;
        let center = blurred_coverage(sigma, [4.0, 4.0], [8, 8], solid);
        let edge = blurred_coverage(sigma, [0.0, 4.0], [8, 8], solid);
        let corner = blurred_coverage(sigma, [0.0, 0.0], [8, 8], solid);
        assert!(center < 1.0 && center > 0.6, "center {center}");
        assert!(edge < center * 0.65 && edge > 0.2, "edge {edge}");
        assert!(corner < edge * 0.7, "corner {corner}");
        // Far outside the tile there is nothing.
        assert_eq!(blurred_coverage(sigma, [-20.0, 4.0], [8, 8], solid), 0.0);
        // Without blur the single sample is returned.
        assert!((blurred_coverage(0.0, [4.0, 4.0], [8, 8], solid) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn single_texel_tile_samples_its_only_texel() {
        // Insets collapse onto the midpoint: every in-range tap reads the one texel.
        let value = blurred_coverage(0.2, [0.5, 0.5], [1, 1], |_, _| 0.8);
        assert!((value - 0.8).abs() < 1e-4, "{value}");
    }
}
