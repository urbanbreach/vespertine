//! Window-level tests of fitted and masked image painting (RND-004) and nearest-neighbor
//! painting (RND-007) on the real GPU renderer: the fidelity scenes, the no-upload mask
//! animation, the same-frame mask measurement, and the painter equivalences.

#[path = "../examples/example_support/image_scene.rs"]
mod image_scene;

use std::{cell::Cell, cell::RefCell, rc::Rc};

use image_scene::*;
use vespertine::{AppContext as _, Corners, ImageAlphaMask, Window, px, size};

fn blend(background: u32, foreground: u32, alpha: f32) -> u32 {
    let (b, f) = (rgb_bytes(background), rgb_bytes(foreground));
    let mix = |b: u8, f: u8| (f32::from(b) + (f32::from(f) - f32::from(b)) * alpha).round() as u32;
    mix(b[0], f[0]) << 16 | mix(b[1], f[1]) << 8 | mix(b[2], f[2])
}

#[test]
fn fid_rnd_004_1_masked_checkerboard_matches_the_mask_reference() {
    let image = fid_004_1_image();
    let mask = fid_004_1_mask();
    let mut scene = PaintView::scene(
        size(px(900.0), px(500.0)),
        FID_004_1_BACKGROUND,
        FID_004_1_OPACITY,
        move |window| paint_fid_004_1(window, &image, Some(mask)),
    );
    let capture = scene.capture();
    assert_eq!(capture.image.dimensions(), (1800, 1000));
    let effects = mask.to_effects(2.0);
    // Cell centers of the magnified checkerboard (each source pixel is 10 logical pixels):
    // the color is the cell color at opacity 0.8 scaled by the mask coverage.
    let mut compared = 0;
    for j in 0..10u32 {
        for i in 0..20u32 {
            let (x, y) = (55.0 + 40.0 * i as f32, 55.0 + 40.0 * j as f32);
            let cell = if (i + j) % 2 == 0 { 0x70B8FF } else { 0xFFC878 };
            let coverage = effects.mask_coverage([x * 2.0, y * 2.0]);
            let expected = blend(FID_004_1_BACKGROUND, cell, 0.8 * coverage);
            assert!(
                near(capture.at(x, y), expected, 4),
                "cell ({i}, {j}) at ({x}, {y}): {:?} vs #{expected:06X}",
                capture.at(x, y)
            );
            compared += 1;
        }
    }
    assert_eq!(compared, 200);
    // Landmarks: the cutout center and the area below the bottom edge show the background;
    // the unmasked left margin shows the checkerboard at 80 percent.
    assert!(near(capture.at(440.0, 225.0), FID_004_1_BACKGROUND, 3));
    assert!(near(capture.at(300.0, 450.0), FID_004_1_BACKGROUND, 3));
    assert!(near(
        capture.at(55.0, 55.0),
        blend(FID_004_1_BACKGROUND, 0x70B8FF, 0.8),
        3
    ));
    // The ordinary corner radius (24) is intact: the image corner is background.
    assert!(near(capture.at(41.0, 41.0), FID_004_1_BACKGROUND, 2));
}

#[test]
fn fid_rnd_004_2_crops_with_intact_corners_and_a_cutout() {
    let image = fid_004_2_image();
    let mask = fid_004_2_mask();
    let mut scene = PaintView::scene(
        size(px(640.0), px(400.0)),
        FID_004_2_BACKGROUND,
        1.0,
        move |window| paint_fid_004_2(window, &image, Some(mask)),
    );
    let capture = scene.capture();
    // The red/blue boundary falls at window x = 320 (crop of 366 texels from texel 73).
    assert!(near(capture.at(300.0, 100.0), 0xE04040, 3));
    assert!(near(capture.at(340.0, 100.0), 0x4080E0, 3));
    assert!(near(capture.device(636, 200), 0xE04040, 3));
    assert!(near(capture.device(645, 200), 0x4080E0, 3));
    // Intact rounding on all four visible corners (radius 24 at (120,60) 400 x 280).
    for (x, y) in [(120.5, 60.5), (519.5, 60.5), (120.5, 339.5), (519.5, 339.5)] {
        assert!(
            near(capture.at(x, y), FID_004_2_BACKGROUND, 2),
            "corner ({x}, {y})"
        );
    }
    for (x, y) in [(128.0, 68.0), (512.0, 68.0), (128.0, 332.0), (512.0, 332.0)] {
        assert!(
            !near(capture.at(x, y), FID_004_2_BACKGROUND, 20),
            "inside ({x}, {y})"
        );
    }
    // The mask removes the center and leaves the far side of the image untouched.
    assert!(near(capture.at(320.0, 190.0), FID_004_2_BACKGROUND, 3));
    assert!(near(capture.at(500.0, 190.0), 0x4080E0, 3));
    let effects = mask.to_effects(2.0);
    for (x, y) in [
        (225.0, 190.0),
        (330.0, 130.0),
        (415.0, 180.0),
        (310.0, 240.0),
    ] {
        let coverage = effects.mask_coverage([x * 2.0, y * 2.0]);
        let source = if x < 320.0 { 0xE04040 } else { 0x4080E0 };
        let expected = blend(FID_004_2_BACKGROUND, source, coverage);
        assert!(
            near(capture.at(x, y), expected, 5),
            "({x}, {y}): {:?} vs #{expected:06X}",
            capture.at(x, y)
        );
    }
}

#[test]
fn fid_rnd_004_3_cover_fit_tall_cutout_with_full_height_bottom_fade() {
    let image = fid_004_3_image();
    let mask = fid_004_3_mask();
    let mut scene = PaintView::scene(
        size(px(800.0), px(480.0)),
        FID_004_3_BACKGROUND,
        1.0,
        move |window| paint_fid_004_3(window, &image, Some(mask)),
    );
    let capture = scene.capture();
    let effects = mask.to_effects(2.0);
    let mut compared = 0;
    for x in [100.0, 150.0, 300.0, 400.0, 500.0, 700.0] {
        for y in [
            45.0, 80.0, 150.0, 230.0, 250.0, 300.0, 360.0, 400.0, 430.0, 438.0,
        ] {
            // Source y = window y - 40: the upper color ends at window y = 240.
            let source = if y < 240.0 { 0x8060A0 } else { 0x40A080 };
            let coverage = effects.mask_coverage([x * 2.0, y * 2.0]);
            let expected = blend(FID_004_3_BACKGROUND, source, coverage);
            assert!(
                near(capture.at(x, y), expected, 4),
                "({x}, {y}): {:?} vs #{expected:06X} (coverage {coverage})",
                capture.at(x, y)
            );
            compared += 1;
        }
    }
    assert_eq!(compared, 60);
    // Alpha 1 at the top of the viewport, 0 at its bottom edge; outside the viewport the
    // background.
    assert!(near(capture.at(400.0, 41.0), 0x8060A0, 4));
    assert!(near(capture.at(400.0, 441.0), FID_004_3_BACKGROUND, 2));
    assert!(near(capture.at(60.0, 200.0), FID_004_3_BACKGROUND, 2));
}

#[test]
fn fid_rnd_007_nearest_blocks_and_linear_copy() {
    let image = fid_007_image();
    let mut scene = PaintView::scene(size(px(400.0), px(200.0)), 0xFFFFFF, 1.0, move |window| {
        paint_fid_007(window, &image)
    });
    check_fid_007(&scene.capture());
}

#[test]
fn animating_the_mask_over_a_stationary_image_uploads_nothing() {
    // RND-004 A5: eight frames with a moving, resizing mask over one image.
    let image = fid_004_3_image();
    let position = Rc::new(Cell::new(0.0f32));
    let paint_position = position.clone();
    let mut scene = PaintView::scene(
        size(px(800.0), px(480.0)),
        FID_004_3_BACKGROUND,
        1.0,
        move |window| {
            let offset = paint_position.get();
            let mask = ImageAlphaMask {
                bounds: logical_bounds(240.0 + offset, 240.0, 320.0 - offset / 2.0, 200.0),
                radius: px(20.0),
                feather: px(30.0 + offset / 4.0),
                clearance: px(8.0),
                bottom_fade: Some((px(440.0), px(100.0))),
            };
            paint_fid_004_3(window, &image, Some(mask));
        },
    );
    scene.draw();
    let warm_insertions = scene.counters.insertions();
    let warm_bytes = scene.counters.uploaded_bytes();
    assert_eq!(warm_insertions, 1);
    assert_eq!(warm_bytes, 800 * 400 * 4);
    let mut first_tile = None;
    for frame in 0..8 {
        let offset = frame as f32 * 12.0;
        position.set(offset);
        scene.draw();
        let sprites = scene.with_window(|window, _| window.painted_polychrome_sprites());
        assert_eq!(sprites.len(), 1);
        let sprite = sprites[0];
        // No stale mask: this frame's parameters, in device pixels.
        assert_eq!(
            sprite.effects.mask_bounds,
            [
                (240.0 + offset) * 2.0,
                480.0,
                (320.0 - offset / 2.0) * 2.0,
                400.0
            ]
        );
        assert_eq!(sprite.effects.mask_feather, (30.0 + offset / 4.0) * 2.0);
        // One texture allocation and the same source pixels all along.
        let tile = *first_tile.get_or_insert(sprite.tile);
        assert_eq!(sprite.tile.texture_id, tile.texture_id);
        assert_eq!(sprite.tile.tile_id, tile.tile_id);
        assert_eq!(scene.counters.insertions(), warm_insertions);
        assert_eq!(scene.counters.uploaded_bytes(), warm_bytes);
    }
    // The rendered pixels follow the mask: the cutout center moved right with the mask.
    position.set(0.0);
    let before = scene.capture();
    position.set(96.0);
    let after = scene.capture();
    assert!(near(before.at(260.0, 340.0), FID_004_3_BACKGROUND, 3));
    assert!(!near(after.at(260.0, 340.0), FID_004_3_BACKGROUND, 12));
    assert_eq!(scene.counters.insertions(), 1);
    assert_eq!(scene.counters.uploaded_bytes(), warm_bytes);
}

#[test]
fn resizing_the_excluded_surface_keeps_the_mask_on_the_same_frames_measurement() {
    // RND-004 A13 / R16: the mask comes from the surface bounds measured in this frame's
    // prepaint, extends to the image bottom, and no image is decoded again.
    let scene_image = resize_scene_image();
    let height = Rc::new(Cell::new(40.0f32));
    let scene_height = height.clone();
    let mut scene = image_scene::HeadlessScene::new(
        size(px(RESIZE_WINDOW.0), px(RESIZE_WINDOW.1)),
        move |_, cx| cx.new(|_| ResizeScene::new(scene_image, scene_height)),
    );
    let image_bottom = (RESIZE_IMAGE_AREA.1 + RESIZE_IMAGE_AREA.3) * 2.0;
    let mut previous_mask_top = None;
    for surface_height in [40.0, 72.0, 104.0, 56.0, 88.0] {
        height.set(surface_height);
        scene.draw();
        let surface = scene.with_window(|window, _| painted_surface_quad(window));
        let sprite = scene.with_window(|window, _| window.painted_polychrome_sprites())[0];
        let mask = sprite.effects.mask_bounds;
        assert_eq!(surface[3], surface_height * 2.0);
        // Same frame: the mask's left edge, top edge and width are the surface's.
        assert_eq!(
            [mask[0], mask[1], mask[2]],
            [surface[0], surface[1], surface[2]]
        );
        // And it extends to the image bottom.
        assert_eq!(mask[1] + mask[3], image_bottom);
        assert_ne!(
            previous_mask_top,
            Some(mask[1]),
            "the mask must follow every resize"
        );
        previous_mask_top = Some(mask[1]);
        assert_eq!(
            scene.counters.insertions(),
            1,
            "no decode and no new image identity"
        );
    }
    assert_eq!(scene.counters.uploaded_bytes(), 800 * 400 * 4);
    // Pixels: just above the surface's top edge region the artwork shows; under the surface
    // (inside the mask) it is gone.
    let capture = scene.capture();
    let artwork_above = capture.at(300.0, 60.0);
    assert!(!near(artwork_above, 0x181A1F, 12));
    assert!(near(capture.at(300.0, 300.0), 0x2A2E38, 3));
    // Below the surface, still inside the exclusion that reaches the image bottom, the
    // artwork does not reappear.
    assert!(near(capture.at(300.0, 330.0), 0x181A1F, 4));
}

#[test]
fn paint_image_equals_the_fitted_painter_when_no_crop_applies() {
    let image = fid_004_2_image();
    let results: Rc<RefCell<Vec<bool>>> = Rc::default();
    let sink = results.clone();
    let mut scene = PaintView::scene(
        size(px(300.0), px(300.0)),
        0x000000,
        1.0,
        move |window: &mut Window| {
            let area = logical_bounds(10.0, 20.0, 100.0, 50.0);
            let radii = Corners::all(px(9.0));
            let mut sink = sink.borrow_mut();
            sink.push(
                window
                    .paint_image(area, area, radii, image.clone(), 0, false)
                    .is_ok(),
            );
            sink.push(
                window
                    .paint_image_fitted(area, area, radii, image.clone(), 0, false)
                    .is_ok(),
            );
            sink.push(
                window
                    .paint_image_fitted_masked(area, area, radii, image.clone(), 0, false, None)
                    .is_ok(),
            );
            // paint_image with image bounds larger than bounds is the fitted painter with
            // visible = bounds ∩ image bounds.
            let bounds = logical_bounds(150.0, 20.0, 60.0, 60.0);
            let image_bounds = logical_bounds(130.0, 0.0, 200.0, 100.0);
            sink.push(
                window
                    .paint_image(bounds, image_bounds, radii, image.clone(), 0, true)
                    .is_ok(),
            );
            sink.push(
                window
                    .paint_image_fitted(
                        bounds.intersect(&image_bounds),
                        image_bounds,
                        radii,
                        image.clone(),
                        0,
                        true,
                    )
                    .is_ok(),
            );
            // Empty rectangles paint nothing and succeed.
            let empty = logical_bounds(10.0, 10.0, 0.0, 10.0);
            sink.push(
                window
                    .paint_image(empty, area, radii, image.clone(), 0, false)
                    .is_ok(),
            );
            sink.push(
                window
                    .paint_image(area, empty, radii, image.clone(), 0, false)
                    .is_ok(),
            );
            sink.push(
                window
                    .paint_image_fitted(empty, area, radii, image.clone(), 0, false)
                    .is_ok(),
            );
            sink.push(
                window
                    .paint_image_fitted(area, empty, radii, image.clone(), 0, false)
                    .is_ok(),
            );
            sink.push(
                window
                    .paint_image_nearest(empty, area, radii, image.clone(), 0, false)
                    .is_ok(),
            );
        },
    );
    scene.draw();
    assert!(
        results.borrow().iter().all(|ok| *ok),
        "{:?}",
        results.borrow()
    );
    let sprites = scene.with_window(|window, _| window.painted_polychrome_sprites());
    assert_eq!(sprites.len(), 5, "empty rectangles paint nothing");
    // Primitives are ordered by overlap layers, not call order: group by position.
    let normalized = |sprite: &vespertine::PolychromeSprite| {
        let mut sprite = *sprite;
        sprite.order = 0;
        format!("{sprite:?}")
    };
    let uncropped: Vec<_> = sprites
        .iter()
        .filter(|s| s.bounds.origin.x.0 == 20.0)
        .collect();
    let cropped: Vec<_> = sprites
        .iter()
        .filter(|s| s.bounds.origin.x.0 == 300.0)
        .collect();
    assert_eq!((uncropped.len(), cropped.len()), (3, 2));
    assert_eq!(normalized(uncropped[0]), normalized(uncropped[1]));
    assert_eq!(normalized(uncropped[0]), normalized(uncropped[2]));
    assert_eq!(normalized(cropped[0]), normalized(cropped[1]));
    // The crop of the larger image bounds is a proper sub-tile.
    assert_ne!(cropped[0].tile.bounds, uncropped[0].tile.bounds);
    // No mask and no nearest flag on ordinary painting.
    assert_eq!(uncropped[0].effects, vespertine::ImageEffects::default());
    assert_eq!(cropped[0].effects, vespertine::ImageEffects::default());
}

#[test]
fn nearest_painting_marks_only_its_own_sprite_and_shares_the_atlas_tile() {
    let image = fid_007_image();
    let mut scene = PaintView::scene(size(px(400.0), px(200.0)), 0xFFFFFF, 1.0, move |window| {
        paint_fid_007(window, &image)
    });
    scene.draw();
    let sprites = scene.with_window(|window, _| window.painted_polychrome_sprites());
    assert_eq!(sprites.len(), 2);
    let nearest = sprites
        .iter()
        .find(|s| s.effects.is_nearest())
        .expect("nearest sprite");
    let linear = sprites
        .iter()
        .find(|s| !s.effects.is_nearest())
        .expect("linear sprite");
    assert_eq!(
        nearest.tile, linear.tile,
        "one atlas tile for both samplers"
    );
    assert_eq!(scene.counters.insertions(), 1, "the image is inserted once");
    assert_eq!(linear.effects, vespertine::ImageEffects::default());
    assert_eq!(nearest.bounds.size.width.0, 240.0);
}

#[test]
fn atlas_insertion_errors_propagate_to_the_caller() {
    let image = fid_007_image();
    let outcome: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = outcome.clone();
    let mut scene = PaintView::scene(size(px(100.0), px(100.0)), 0x000000, 1.0, move |window| {
        let area = logical_bounds(0.0, 0.0, 50.0, 50.0);
        for result in [
            window.paint_image(area, area, Corners::default(), image.clone(), 0, false),
            window.paint_image_nearest(area, area, Corners::default(), image.clone(), 0, false),
            window.paint_image_fitted(area, area, Corners::default(), image.clone(), 0, false),
            window.paint_image_fitted_masked(
                area,
                area,
                Corners::default(),
                image.clone(),
                0,
                false,
                None,
            ),
        ] {
            sink.borrow_mut().push(format!("{result:?}"));
        }
    });
    // The window's first draw (at creation) succeeded; start counting from the failing one.
    outcome.borrow_mut().clear();
    scene.counters.set_failing(true);
    scene.draw();
    let outcome = outcome.borrow();
    assert_eq!(outcome.len(), 4);
    for message in outcome.iter() {
        assert!(
            message.contains("the atlas cannot allocate the image"),
            "{message}"
        );
    }
    assert!(
        scene
            .with_window(|window, _| window.painted_polychrome_sprites())
            .is_empty()
    );
}
