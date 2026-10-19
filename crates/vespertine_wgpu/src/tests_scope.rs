//! Pixel tests of the scoped edge fade, rounded clip and curved overflow clip (RND-001,
//! RND-002) through the headless renderer, compared with the reference arithmetic in
//! `vespertine::ScopeParams` and `vespertine::CurvedClipShape`.

use crate::WgpuHeadlessRenderer;
use image::RgbaImage;
use vespertine::{
    Background, BorderStyle, Bounds, ContentMask, Corners, CurvedClipShape, DevicePixels, Edges,
    Hsla, PlatformHeadlessRenderer, Point, Quad, ScaledPixels, Scene, ScopeParams, Size, white,
};

pub(crate) fn device_size(width: i32, height: i32) -> Size<DevicePixels> {
    Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    }
}

pub(crate) fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: Point {
            x: ScaledPixels(x),
            y: ScaledPixels(y),
        },
        size: Size {
            width: ScaledPixels(width),
            height: ScaledPixels(height),
        },
    }
}

pub(crate) fn quad(
    area: Bounds<ScaledPixels>,
    background: impl Into<Background>,
    scope: ScopeParams,
) -> Quad {
    Quad {
        order: 0,
        border_style: BorderStyle::Solid,
        bounds: area,
        content_mask: ContentMask { bounds: area },
        background: background.into(),
        border_color: Hsla::default(),
        corner_radii: Corners::default(),
        border_widths: Edges::default(),
        scope,
    }
}

pub(crate) fn render(scene: &mut Scene, width: i32, height: i32) -> RgbaImage {
    scene.finish();
    let mut renderer = WgpuHeadlessRenderer::new().expect("headless renderer");
    renderer
        .render_scene_to_image(scene, device_size(width, height))
        .expect("render")
}

pub(crate) fn gradient(color: Hsla) -> Background {
    vespertine::linear_gradient(
        180.0,
        vespertine::linear_color_stop(color, 0.0),
        vespertine::linear_color_stop(color, 1.0),
    )
}

/// Red channel of a pixel as a coverage in `[0, 1]` (white drawn over black).
pub(crate) fn coverage(image: &RgbaImage, x: u32, y: u32) -> f32 {
    f32::from(image.get_pixel(x, y).0[0]) / 255.0
}

#[test]
fn edge_fade_matches_reference_on_every_pixel() {
    let scope = ScopeParams {
        fade_edges: [4.0, 60.0, 3.0, 62.0],
        fade_bands: [20.0, 8.0, 12.0, 30.0],
        ..Default::default()
    };
    let mut scene = Scene::default();
    scene.insert_primitive(quad(bounds(0.0, 0.0, 64.0, 64.0), white(), scope));
    let image = render(&mut scene, 64, 64);
    for y in 0..64 {
        for x in 0..64 {
            let expected = scope.factor([x as f32 + 0.5, y as f32 + 0.5]);
            let actual = coverage(&image, x, y);
            assert!(
                (actual - expected).abs() <= 2.5 / 255.0,
                "({x}, {y}): rendered {actual}, expected {expected}"
            );
        }
    }
}

#[test]
fn plain_solid_bypass_is_decided_per_batch_not_per_quad() {
    // Rounded scope only: no fade band, so a plain solid quad bypasses K (RND-002 R6).
    let scope = ScopeParams {
        clip_bounds: [8.0, 8.0, 56.0, 56.0],
        clip_radius: 24.0,
        ..Default::default()
    };
    let area = bounds(0.0, 0.0, 64.0, 64.0);
    let mut plain = Scene::default();
    plain.insert_primitive(quad(area, white(), scope));
    let image = render(&mut plain, 64, 64);
    // The cut-away corner and the region outside the clip bounds stay fully painted.
    assert!((coverage(&image, 9, 9) - 1.0).abs() < 0.02);
    assert!((coverage(&image, 2, 2) - 1.0).abs() < 0.02);

    // A gradient fill takes the general route: K applies.
    let mut general = Scene::default();
    general.insert_primitive(quad(area, gradient(white()), scope));
    let image = render(&mut general, 64, 64);
    assert!(coverage(&image, 9, 9) < 0.02, "corner must be cut");
    assert!(coverage(&image, 2, 2) < 0.02, "outside must be cut");
    assert!((coverage(&image, 32, 32) - 1.0).abs() < 0.02);
    let expected = scope.rounded_coverage([12.5, 12.5]);
    assert!((coverage(&image, 12, 12) - expected).abs() <= 2.5 / 255.0);

    // PRF-008 R3, RND-001 R11a (amended): the same plain solid quad sharing its draw batch with
    // a gradient quad is drawn by the general route and receives K. The two quads do not
    // overlap, so the scene's batching puts them in one batch.
    let far = ScopeParams {
        clip_bounds: [136.0, 8.0, 184.0, 56.0],
        clip_radius: 24.0,
        ..Default::default()
    };
    let mut mixed = Scene::default();
    mixed.insert_primitive(quad(area, white(), scope));
    mixed.insert_primitive(quad(bounds(128.0, 0.0, 64.0, 64.0), gradient(white()), far));
    let image = render(&mut mixed, 192, 64);
    assert!(
        coverage(&image, 9, 9) < 0.02,
        "corner must be cut in a mixed batch"
    );
    assert!(
        coverage(&image, 2, 2) < 0.02,
        "outside must be cut in a mixed batch"
    );
    assert!((coverage(&image, 32, 32) - 1.0).abs() < 0.02);
    assert!((coverage(&image, 12, 12) - expected).abs() <= 2.5 / 255.0);
    assert!(coverage(&image, 137, 9) < 0.02);
}

#[test]
fn rounded_quad_fill_receives_scope_factor_twice_and_interior_once() {
    // F = 0.5 everywhere on the probed rows: edge ramp u^2 = 0.5.
    let band = 70.5 / std::f32::consts::FRAC_1_SQRT_2;
    let scope = ScopeParams {
        fade_edges: [0.0, 0.0, 0.0, 0.0],
        fade_bands: [band, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let mut rounded = quad(bounds(0.0, 0.0, 64.0, 128.0), white(), scope);
    rounded.corner_radii = Corners::all(ScaledPixels(24.0));
    let mut scene = Scene::default();
    scene.insert_primitive(rounded);
    let image = render(&mut scene, 64, 128);
    // y = 70: far from every corner, inside the interior fast return: F once.
    let interior = scope.factor([32.5, 70.5]);
    assert!((interior - 0.5).abs() < 0.01);
    assert!((coverage(&image, 32, 70) - 0.5).abs() <= 3.0 / 255.0);
    // Inside the rounded corner region (but inside the curve) the fill gets F twice.
    let corner = scope.factor([9.5, 9.5]);
    assert!(corner < 0.1);
    let probe_y = 70u32;
    let mut rounded = quad(bounds(0.0, 56.0, 64.0, 72.0), white(), scope);
    rounded.corner_radii = Corners::all(ScaledPixels(24.0));
    let mut scene = Scene::default();
    scene.insert_primitive(rounded);
    let image = render(&mut scene, 64, 128);
    // Pixel (12, 70) lies in the top-left corner region of this quad, inside the curve.
    let f = scope.factor([12.5, probe_y as f32 + 0.5]);
    assert!((f - 0.5).abs() < 0.01);
    assert!(
        (coverage(&image, 12, probe_y) - f * f).abs() <= 3.0 / 255.0,
        "fill must carry F twice in the rounded corner region: {} vs {}",
        coverage(&image, 12, probe_y),
        f * f
    );
}

#[test]
fn curved_clip_chain_combines_by_minimum_on_plain_quads() {
    let outer = CurvedClipShape {
        bounds: [4.0, 4.0, 60.0, 60.0],
        radii_x: [20.0, 8.0, 20.0, 8.0],
        radii_y: [12.0, 8.0, 16.0, 8.0],
    };
    let inner = CurvedClipShape {
        bounds: [20.0, 0.0, 70.0, 50.0],
        radii_x: [10.0; 4],
        radii_y: [10.0; 4],
    };
    let mut scene = Scene::default();
    scene.push_curved_clip(outer);
    scene.push_curved_clip(inner);
    scene.insert_primitive(quad(
        bounds(0.0, 0.0, 64.0, 64.0),
        white(),
        Default::default(),
    ));
    scene.pop_curved_clip();
    scene.pop_curved_clip();
    let image = render(&mut scene, 64, 64);
    for y in 0..64 {
        for x in 0..64 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let expected = vespertine::curved_clip_coverage([&outer, &inner], p);
            assert!(
                (coverage(&image, x, y) - expected).abs() <= 2.5 / 255.0,
                "({x}, {y}): {} vs {expected}",
                coverage(&image, x, y)
            );
        }
    }
}
