//! Tests of the adapter-aware solid-fill specialization (PRF-008): the per-batch route choice
//! on hardware adapters, the exact opaque and large-interior guards, and the run and split
//! behavior of software adapters.

use super::WgpuHeadlessRenderer;
use super::recording::{
    FillClass, FillRecord, is_opaque_solid, is_simple_solid, large_interior, quad_fill_class,
};
use crate::tests_scope::{bounds, device_size, gradient, quad};
use image::RgbaImage;
use vespertine::{
    Corners, Edges, Hsla, PlatformHeadlessRenderer as _, Quad, ScaledPixels, Scene, ScopeParams,
    black, white,
};

fn translucent() -> Hsla {
    white().opacity(0.5)
}

fn renderer(software: bool) -> WgpuHeadlessRenderer {
    let mut renderer = WgpuHeadlessRenderer::new().expect("headless renderer");
    renderer.record_quad_fills(true);
    renderer.set_software_quad_fills(software);
    renderer
}

fn render(renderer: &mut WgpuHeadlessRenderer, scene: &mut Scene, w: i32, h: i32) -> RgbaImage {
    scene.finish();
    renderer
        .render_scene_to_image(scene, device_size(w, h))
        .expect("render")
}

fn solid(x: f32, y: f32, w: f32, h: f32, color: Hsla) -> Quad {
    quad(bounds(x, y, w, h), color, ScopeParams::default())
}

fn rounded(x: f32, y: f32, w: f32, h: f32, color: Hsla, radius: f32) -> Quad {
    let mut quad = solid(x, y, w, h, color);
    quad.corner_radii = Corners::all(ScaledPixels(radius));
    quad
}

fn bordered(x: f32, y: f32, w: f32, h: f32, color: Hsla) -> Quad {
    let mut quad = solid(x, y, w, h, color);
    quad.border_widths = Edges::all(ScaledPixels(2.0));
    quad.border_color = black();
    quad
}

fn fills(renderer: &WgpuHeadlessRenderer) -> Vec<FillRecord> {
    renderer.quad_fills().to_vec()
}

#[test]
fn hardware_batches_pick_one_route_and_stay_whole() {
    // A1: an unclipped opaque solid batch selects the opaque route.
    let mut hardware = renderer(false);
    let mut scene = Scene::default();
    for column in 0..4 {
        scene.insert_primitive(solid(column as f32 * 20.0, 0.0, 16.0, 16.0, white()));
    }
    render(&mut hardware, &mut scene, 80, 16);
    let records = fills(&hardware);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].class, FillClass::OpaqueSolid);
    assert_eq!(records[0].quads, 0..4);
    assert_eq!((records[0].draws, records[0].scissors), (1, 0));

    // A translucent simple solid batch selects the clipped solid route.
    let mut scene = Scene::default();
    for column in 0..4 {
        scene.insert_primitive(solid(column as f32 * 20.0, 0.0, 16.0, 16.0, translucent()));
    }
    render(&mut hardware, &mut scene, 80, 16);
    let records = fills(&hardware);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].class, FillClass::ClippedSolid);

    // A batch mixing opaque, bordered, rounded and gradient members stays one batch and uses
    // general shading.
    let mut scene = Scene::default();
    scene.insert_primitive(solid(0.0, 0.0, 16.0, 16.0, white()));
    scene.insert_primitive(bordered(20.0, 0.0, 16.0, 16.0, white()));
    scene.insert_primitive(rounded(40.0, 0.0, 16.0, 16.0, white(), 4.0));
    scene.insert_primitive(quad(
        bounds(60.0, 0.0, 16.0, 16.0),
        gradient(white()),
        ScopeParams::default(),
    ));
    render(&mut hardware, &mut scene, 80, 16);
    let records = fills(&hardware);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].class, FillClass::General);
    assert_eq!(records[0].quads, 0..4);
    assert_eq!((records[0].draws, records[0].scissors), (1, 0));
}

#[test]
fn a_fade_band_or_a_curved_clip_keeps_a_quad_off_the_solid_routes() {
    // A positive fade band makes a quad general; a zero band does not. The rounded clip fields
    // are never inspected.
    let mut faded = solid(0.0, 0.0, 16.0, 16.0, white());
    faded.scope.fade_bands = [4.0, 0.0, 0.0, 0.0];
    assert!(!is_simple_solid(&faded));
    let mut zero_band = solid(0.0, 0.0, 16.0, 16.0, white());
    zero_band.scope.fade_bands = [0.0, -3.0, 0.0, 0.0];
    assert!(is_simple_solid(&zero_band));
    let mut clipped = solid(0.0, 0.0, 16.0, 16.0, white());
    clipped.scope.clip_bounds = [2.0, 2.0, 10.0, 10.0];
    clipped.scope.clip_radius = 4.0;
    assert!(is_simple_solid(&clipped));
    assert_eq!(quad_fill_class(&clipped), FillClass::OpaqueSolid);
    // A curved overflow chain needs the per-fragment route even for an opaque quad.
    let mut in_chain = solid(0.0, 0.0, 16.0, 16.0, white());
    in_chain.scope.curved_clip = 1;
    assert!(is_simple_solid(&in_chain));
    assert!(!is_opaque_solid(&in_chain));
    assert_eq!(quad_fill_class(&in_chain), FillClass::ClippedSolid);
}

#[test]
fn opaque_requires_alpha_exactly_one_and_the_ceil_floor_envelope_inside_the_mask() {
    let opaque = solid(4.0, 4.0, 8.0, 8.0, white());
    assert!(is_opaque_solid(&opaque));
    // Alpha just below one is translucent.
    let almost = solid(4.0, 4.0, 8.0, 8.0, white().opacity(0.999_999));
    assert!(!is_opaque_solid(&almost));
    // A2: the floating-point bounds are inside the mask but the floor/ceil envelope is not.
    let mut fractional = solid(4.5, 4.5, 7.0, 7.0, white());
    fractional.content_mask.bounds = bounds(4.5, 4.5, 7.0, 7.0);
    assert!(!is_opaque_solid(&fractional));
    assert_eq!(quad_fill_class(&fractional), FillClass::ClippedSolid);
    // Integer bounds inside a larger mask are fine; a mask that cuts the quad is not.
    let mut inside = solid(4.0, 4.0, 8.0, 8.0, white());
    inside.content_mask.bounds = bounds(0.0, 0.0, 32.0, 32.0);
    assert!(is_opaque_solid(&inside));
    let mut cut = solid(4.0, 4.0, 8.0, 8.0, white());
    cut.content_mask.bounds = bounds(6.0, 0.0, 32.0, 32.0);
    assert!(!is_opaque_solid(&cut));
}

#[test]
fn degenerate_or_out_of_range_geometry_never_takes_the_opaque_shortcut() {
    // A3.
    for (x, y, w, h) in [
        (f32::NAN, 0.0, 8.0, 8.0),
        (0.0, f32::INFINITY, 8.0, 8.0),
        (0.0, 0.0, f32::NAN, 8.0),
        (0.0, 0.0, 0.0, 8.0),
        (0.0, 0.0, 8.0, -4.0),
        (1_048_577.0, 0.0, 8.0, 8.0),
        (0.0, 0.0, 2_000_000.0, 8.0),
    ] {
        let mut quad = solid(x, y, w, h, white());
        quad.content_mask.bounds = bounds(-4_000_000.0, -4_000_000.0, 8_000_000.0, 8_000_000.0);
        assert!(
            !is_opaque_solid(&quad),
            "({x}, {y}, {w}, {h}) must not be opaque"
        );
    }
    // Non-finite or out-of-range inputs of the large-interior test use general shading.
    let drawable = (4096, 4096);
    let template = rounded(0.0, 0.0, 400.0, 400.0, white(), 8.0);
    assert!(large_interior(&template, drawable).is_some());
    let mut non_finite = template;
    non_finite.corner_radii = Corners::all(ScaledPixels(f32::NAN));
    assert!(large_interior(&non_finite, drawable).is_none());
    let mut infinite_mask = template;
    infinite_mask.content_mask.bounds = bounds(0.0, 0.0, f32::INFINITY, 400.0);
    assert!(large_interior(&infinite_mask, drawable).is_none());
    let mut far = template;
    far.bounds.origin.x = ScaledPixels(1_048_577.0);
    far.content_mask.bounds = bounds(1_048_000.0, 0.0, 2_000.0, 400.0);
    assert!(large_interior(&far, drawable).is_none());
    let mut faded = template;
    faded.scope.fade_edges = [0.0, 2_000_000.0, 0.0, 0.0];
    faded.scope.fade_bands = [0.0, 16.0, 0.0, 0.0];
    assert!(large_interior(&faded, drawable).is_none());
    // Simple solid quads are never candidates.
    assert!(large_interior(&solid(0.0, 0.0, 400.0, 400.0, white()), drawable).is_none());
}

#[test]
fn the_large_interior_threshold_is_inclusive_at_16384_pixels() {
    // A4. Radius 1 gives g = 2, so the interior is the bounds inset by two on every side.
    let drawable = (512, 512);
    let at_threshold = rounded(0.0, 0.0, 132.0, 132.0, white(), 1.0);
    assert_eq!(
        large_interior(&at_threshold, drawable),
        Some([2, 2, 130, 130]),
        "128 x 128 = 16384 is accepted"
    );
    let below = rounded(0.0, 0.0, 133.0, 131.0, white(), 1.0);
    assert_eq!(
        large_interior(&below, drawable),
        None,
        "129 x 127 = 16383 is rejected"
    );
}

#[test]
fn the_interior_accounts_for_borders_fade_bands_the_mask_and_the_drawable() {
    let drawable = (400, 400);
    // g = max(radius 6, border 10) + 1 = 11, fractional origin rounds inward.
    let mut quad = rounded(20.25, 20.25, 300.0, 300.0, white(), 6.0);
    quad.border_widths = Edges::all(ScaledPixels(10.0));
    assert_eq!(large_interior(&quad, drawable), Some([32, 32, 309, 309]));
    // A fade band moves the interior to one pixel beyond the band's inner endpoint.
    let mut faded = rounded(0.0, 0.0, 300.0, 300.0, white(), 2.0);
    faded.scope.fade_edges = [0.0, 300.0, 0.0, 300.0];
    faded.scope.fade_bands = [40.0, 0.0, 0.0, 30.0];
    assert_eq!(large_interior(&faded, drawable), Some([3, 41, 269, 297]));
    // The content mask and the drawable clamp the interior.
    let mut masked = rounded(0.0, 0.0, 300.0, 300.0, white(), 2.0);
    masked.content_mask.bounds = bounds(50.0, 60.0, 100.0, 200.0);
    assert_eq!(large_interior(&masked, drawable), Some([50, 60, 150, 260]));
    let overhang = rounded(-100.0, -100.0, 600.0, 600.0, white(), 2.0);
    let mut overhang = overhang;
    overhang.content_mask.bounds = bounds(-100.0, -100.0, 600.0, 600.0);
    assert_eq!(large_interior(&overhang, drawable), Some([0, 0, 400, 400]));
}

/// Quads of every class over a checkerboard of opaque and translucent solids, bordered and
/// rounded panels and a gradient, in paint order, overlapping so that order matters.
fn mixed_scene() -> Scene {
    let mut scene = Scene::default();
    scene.insert_primitive(solid(0.0, 0.0, 400.0, 300.0, white().opacity(0.25)));
    scene.insert_primitive(solid(
        10.0,
        10.0,
        200.0,
        200.0,
        Hsla {
            h: 0.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        },
    ));
    let mut panel = rounded(
        20.3,
        20.7,
        300.0,
        240.2,
        Hsla {
            h: 0.6,
            s: 0.8,
            l: 0.5,
            a: 0.5,
        },
        12.0,
    );
    panel.border_widths = Edges::all(ScaledPixels(3.0));
    panel.border_color = black();
    scene.insert_primitive(panel);
    scene.insert_primitive(quad(
        bounds(40.0, 40.0, 160.0, 160.0),
        gradient(Hsla {
            h: 0.3,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        }),
        ScopeParams::default(),
    ));
    let mut faded = rounded(
        60.0,
        60.0,
        280.0,
        220.0,
        Hsla {
            h: 0.1,
            s: 0.9,
            l: 0.6,
            a: 1.0,
        },
        8.0,
    );
    faded.scope.fade_edges = [60.0, 280.0, 60.0, 340.0];
    faded.scope.fade_bands = [30.0, 30.0, 30.0, 30.0];
    scene.insert_primitive(faded);
    scene.insert_primitive(solid(
        120.0,
        120.0,
        60.0,
        60.0,
        Hsla {
            h: 0.8,
            s: 1.0,
            l: 0.5,
            a: 0.5,
        },
    ));
    scene
}

fn max_difference(a: &RgbaImage, b: &RgbaImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels()
        .zip(b.pixels())
        .flat_map(|(a, b)| a.0.into_iter().zip(b.0).map(|(a, b)| a.abs_diff(b)))
        .max()
        .unwrap_or(0)
}

#[test]
fn software_runs_keep_paint_order_and_pixels_of_the_hardware_route() {
    // A6: opaque, translucent, bordered, rounded, gradient and faded quads keep their paint
    // order and their pixels.
    let mut software = renderer(true);
    let mut hardware = renderer(false);
    let software_image = render(&mut software, &mut mixed_scene(), 400, 300);
    let hardware_image = render(&mut hardware, &mut mixed_scene(), 400, 300);
    assert!(
        max_difference(&software_image, &hardware_image) <= 2,
        "software and hardware routes differ by {}",
        max_difference(&software_image, &hardware_image)
    );
    let records = fills(&software);
    // The records cover every quad once, in paint order.
    let mut next = 0;
    for record in &records {
        assert_eq!(record.quads.start, next);
        next = record.quads.end;
    }
    assert_eq!(next, 6);
    // The translucent bordered rounded panel, the gradient quad and the faded quad do not
    // form one run; the faded and the panel quads are large enough to split.
    assert!(records.iter().any(|record| record.split));
    // The hardware frame is one general batch.
    assert_eq!(fills(&hardware).len(), 1);
    assert_eq!(fills(&hardware)[0].class, FillClass::General);
}

#[test]
fn a_split_quad_covers_every_pixel_once_and_restores_the_scissor() {
    // A5: a translucent bordered rounded quad at a fractional origin is shaded identically
    // to general shading, no pixel is blended twice, and the quad drawn after it is not cut
    // by the interior scissor.
    let build = || {
        let mut scene = Scene::default();
        let mut panel = rounded(
            20.4,
            20.6,
            300.3,
            200.1,
            Hsla {
                h: 0.6,
                s: 0.8,
                l: 0.5,
                a: 0.5,
            },
            14.0,
        );
        panel.border_widths = Edges::all(ScaledPixels(3.0));
        panel.border_color = black();
        scene.insert_primitive(panel);
        // Overlaps the panel and reaches outside it, so it is drawn after the split quad.
        scene.insert_primitive(solid(
            10.0,
            10.0,
            100.0,
            40.0,
            Hsla {
                h: 0.0,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            },
        ));
        scene
    };
    let mut software = renderer(true);
    let split_image = render(&mut software, &mut build(), 340, 240);
    let mut general = renderer(false);
    let general_image = render(&mut general, &mut build(), 340, 240);
    assert!(
        max_difference(&split_image, &general_image) <= 2,
        "split and general shading differ by {}",
        max_difference(&split_image, &general_image)
    );
    let records = fills(&software);
    assert_eq!(records.len(), 2, "{records:?}");
    let split = &records[0];
    assert!(split.split);
    assert_eq!(split.quads, 0..1);
    // Four exterior strips and one interior at most; the restoring scissor is counted.
    assert!(split.draws <= 5 && split.draws >= 2, "{split:?}");
    assert_eq!(split.scissors, split.draws + 1);
    // The later quad is a plain run under the restored full scissor: its red covers the strip
    // area outside the interior.
    assert_eq!(records[1].quads, 1..2);
    assert!(!records[1].split);
    let red = split_image.get_pixel(12, 12).0;
    assert!(red[0] > 240 && red[1] < 20, "{red:?}");
}

#[test]
fn clipped_solid_clips_binarily_and_premultiplies_only_on_premultiplied_targets() {
    // A7: a translucent solid quad larger than its content mask takes the clipped route; the
    // mask edge is a hard cut, and the output is scaled by alpha only on a premultiplied
    // target.
    let build = || {
        let mut scene = Scene::default();
        let mut quad = solid(
            0.0,
            0.0,
            32.0,
            16.0,
            Hsla {
                h: 0.0,
                s: 1.0,
                l: 0.5,
                a: 0.5,
            },
        );
        quad.content_mask.bounds = bounds(4.0, 0.0, 16.0, 16.0);
        scene.insert_primitive(quad);
        scene
    };
    let mut straight = renderer(false);
    let image = render(&mut straight, &mut build(), 32, 16);
    assert_eq!(fills(&straight)[0].class, FillClass::ClippedSolid);
    // Binary clip: the pixels just outside the mask keep the black clear color, the pixels
    // just inside carry the blended color.
    assert_eq!(image.get_pixel(3, 8).0, [0, 0, 0, 255]);
    assert_eq!(image.get_pixel(20, 8).0, [0, 0, 0, 255]);
    let inside = image.get_pixel(4, 8).0;
    let inside_last = image.get_pixel(19, 8).0;
    assert_eq!(inside, inside_last);
    // Straight-alpha blending over black: half of the red, once.
    assert!((i32::from(inside[0]) - 128).abs() <= 2, "{inside:?}");

    let mut premultiplied = WgpuHeadlessRenderer::new_transparent().expect("renderer");
    premultiplied.record_quad_fills(true);
    let image = render(&mut premultiplied, &mut build(), 32, 16);
    assert_eq!(fills(&premultiplied)[0].class, FillClass::ClippedSolid);
    assert_eq!(image.get_pixel(3, 8).0, [0, 0, 0, 0]);
    let inside = image.get_pixel(10, 8).0;
    // Premultiplied: color scaled by alpha, alpha 0.5.
    assert!((i32::from(inside[0]) - 128).abs() <= 2, "{inside:?}");
    assert!((i32::from(inside[3]) - 128).abs() <= 2, "{inside:?}");
}
