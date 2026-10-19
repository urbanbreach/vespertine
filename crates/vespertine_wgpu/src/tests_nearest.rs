//! Pixel tests of explicit nearest-neighbor image painting (RND-007) through the production
//! path: `Window::paint_image_nearest`, the scene, the wgpu renderer and its shader.

use crate::tests_host::RenderHost;
use crate::tests_image_mask::{logical, multi_frame_image, painter, rgba_image};
use image::RgbaImage;
use std::sync::Arc;
use vespertine::{Corners, RenderImage, Window, px};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

fn rgb(texel: [u8; 4]) -> [u8; 3] {
    [texel[0], texel[1], texel[2]]
}

fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = image.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

fn close(a: [u8; 3], b: [u8; 3], tolerance: u8) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(b) <= tolerance)
}

fn fid_image() -> Arc<RenderImage> {
    rgba_image(2, 2, &[RED, GREEN, BLUE, WHITE])
}

/// Paints `image` nearest at (40,40) 120 x 120 and linear at (220,40) 120 x 120 (the layout of
/// FID-RND-007) in a 400 x 200 window at scale factor 2.
fn render_pair(
    image: Arc<RenderImage>,
    frame: usize,
    grayscale: bool,
    opacity: f32,
    radii: Corners<vespertine::Pixels>,
) -> RgbaImage {
    let mut host = RenderHost::new();
    host.render_logical(400.0, 200.0, move |_, _| {
        let image = image.clone();
        painter(opacity, move |window: &mut Window| {
            let nearest = logical(40.0, 40.0, 120.0, 120.0);
            let linear = logical(220.0, 40.0, 120.0, 120.0);
            window
                .paint_image_nearest(nearest, nearest, radii, image.clone(), frame, grayscale)
                .expect("nearest paints");
            window
                .paint_image(linear, linear, radii, image.clone(), frame, grayscale)
                .expect("linear paints");
        })
    })
}

#[test]
fn nearest_copy_has_flat_blocks_and_linear_copy_is_smooth() {
    // FID-RND-007: nearest blocks of 120 x 120 device pixels at device (80,80), linear copy
    // at device (440,80).
    let rendered = render_pair(fid_image(), 0, false, 1.0, Corners::default());
    assert_eq!(rendered.dimensions(), (800, 400));

    let block_color = |x: u32, y: u32| match (x >= 200, y >= 200) {
        (false, false) => rgb(RED),
        (true, false) => rgb(GREEN),
        (false, true) => rgb(BLUE),
        (true, true) => rgb(WHITE),
    };
    for y in 81..319 {
        for x in 81..319 {
            let got = pixel(&rendered, x, y);
            assert!(
                close(got, block_color(x, y), 1),
                "nearest ({x}, {y}): {got:?} is not the flat block color {:?}",
                block_color(x, y)
            );
        }
    }
    // The edge between blocks is hard: adjacent device pixels are the two block colors.
    assert!(close(pixel(&rendered, 199, 120), rgb(RED), 1));
    assert!(close(pixel(&rendered, 200, 120), rgb(GREEN), 1));
    assert!(close(pixel(&rendered, 120, 199), rgb(RED), 1));
    assert!(close(pixel(&rendered, 120, 200), rgb(BLUE), 1));

    // The linear copy: a linear blend along the top texel row and the average color in the
    // middle. The outer half texel ring is not flat: the unchanged atlas sampler blends the
    // edge texels with the empty atlas next to the tile (RND-007), so it is not asserted.
    let middle = pixel(&rendered, 560, 200);
    assert!(close(middle, [128, 128, 128], 3), "middle {middle:?}");
    let mut previous_green = 0u8;
    let mut intermediate = 0;
    for x in 500..620 {
        let [r, g, b] = pixel(&rendered, x, 100);
        assert!(
            g >= previous_green,
            "green must increase monotonically at x = {x}"
        );
        previous_green = g;
        assert!(
            r.abs_diff(255 - g) <= 4 && b <= 3,
            "x = {x}: ({r}, {g}, {b})"
        );
        if g > 8 && g < 247 {
            intermediate += 1;
        }
    }
    assert!(
        intermediate > 100,
        "only {intermediate} intermediate columns"
    );
}

#[test]
fn nearest_and_linear_agree_at_texel_centers_for_frames_grayscale_and_opacity() {
    let frame0 = [RED, GREEN, BLUE, WHITE];
    let frame1 = [
        [240, 200, 10, 255],
        [20, 120, 230, 255],
        [90, 90, 40, 255],
        [10, 10, 10, 255],
    ];
    let image = multi_frame_image(2, 2, &[&frame0, &frame1]);
    let centers = [(140u32, 140u32), (260, 140), (140, 260), (260, 260)];
    for (frame, grayscale, opacity) in [
        (0usize, false, 1.0f32),
        (1, false, 1.0),
        (1, true, 1.0),
        (1, false, 0.5),
        (1, true, 0.5),
    ] {
        let rendered = render_pair(image.clone(), frame, grayscale, opacity, Corners::default());
        for (index, (cx, cy)) in centers.into_iter().enumerate() {
            let nearest = pixel(&rendered, cx, cy);
            let linear = pixel(&rendered, cx + 360, cy);
            assert!(
                close(nearest, linear, 4),
                "frame {frame} gray {grayscale} opacity {opacity} texel {index}: nearest {nearest:?} vs linear {linear:?}"
            );
            if !grayscale && opacity == 1.0 {
                let texel = if frame == 0 {
                    frame0[index]
                } else {
                    frame1[index]
                };
                assert!(
                    close(nearest, rgb(texel), 1),
                    "frame {frame} texel {index}: {nearest:?}"
                );
            }
            if grayscale {
                assert!(
                    nearest[0].abs_diff(nearest[1]) <= 1 && nearest[1].abs_diff(nearest[2]) <= 1,
                    "grayscale must be gray: {nearest:?}"
                );
            }
        }
    }
    // Frame selection really picks the frame: the two frames differ at the first texel.
    let first = render_pair(image.clone(), 0, false, 1.0, Corners::default());
    let second = render_pair(image, 1, false, 1.0, Corners::default());
    assert_ne!(pixel(&first, 140, 140), pixel(&second, 140, 140));
}

#[test]
fn nearest_rounded_corners_use_the_same_coverage_as_linear() {
    // Radius 40 logical = 80 device on a 240 device pixel square. The uniform white image
    // makes the texel selection irrelevant: coverage alone shows. The top-left corner is
    // every corner of the nearest copy matches the signed-distance coverage, and the linear
    // copy is never brighter than it: the unchanged atlas sampler blends its outer half
    // texel toward the empty atlas next to the tile (RND-007), so its edge ring is dimmer.
    let white = rgba_image(2, 2, &[WHITE; 4]);
    let rendered = render_pair(white, 0, false, 1.0, Corners::all(px(40.0)));
    for y in 0..120 {
        for x in 0..120 {
            let nearest = rendered.get_pixel(80 + x, 80 + y).0[0];
            let linear = rendered.get_pixel(440 + x, 80 + y).0[0];
            assert!(
                linear <= nearest.saturating_add(1) && (nearest > 0 || linear == 0),
                "corner pixel ({x}, {y}): nearest {nearest}, linear {linear}"
            );
        }
    }
    let radius = 80.0f32;
    for y in 80..320u32 {
        for x in 80..320u32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let q = [
                (p[0] - 200.0).abs() - 120.0 + radius,
                (p[1] - 200.0).abs() - 120.0 + radius,
            ];
            let sdf = (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt()
                + q[0].max(q[1]).min(0.0)
                - radius;
            let want = (0.5 - sdf).clamp(0.0, 1.0);
            let got = f32::from(rendered.get_pixel(x, y).0[0]) / 255.0;
            assert!(
                (got - want).abs() <= 3.0 / 255.0,
                "({x}, {y}): {got} vs {want}"
            );
        }
    }
}

#[test]
fn nearest_crop_uses_the_same_sub_tile_as_linear_painting() {
    // A 4 x 1 image painted into the middle half of its image bounds shows texels 1 and 2
    // as two hard blocks of 100 logical pixels each.
    let image = rgba_image(4, 1, &[RED, GREEN, BLUE, WHITE]);
    let mut host = RenderHost::new();
    let rendered = host.render_logical(400.0, 100.0, move |_, _| {
        let image = image.clone();
        painter(1.0, move |window| {
            window
                .paint_image_nearest(
                    logical(100.0, 0.0, 200.0, 100.0),
                    logical(0.0, 0.0, 400.0, 100.0),
                    Corners::default(),
                    image.clone(),
                    0,
                    false,
                )
                .expect("paints");
        })
    });
    for y in 2..198 {
        for x in 202..598 {
            let want = if x < 400 { rgb(GREEN) } else { rgb(BLUE) };
            assert!(close(pixel(&rendered, x, y), want, 1), "({x}, {y})");
        }
    }
    assert!(close(pixel(&rendered, 399, 100), rgb(GREEN), 1));
    assert!(close(pixel(&rendered, 400, 100), rgb(BLUE), 1));
}

#[test]
fn nearest_and_linear_images_keep_paint_order_in_either_sequence() {
    // RND-007 A5: an opaque red image and an opaque blue image overlap; whichever is painted
    // last is on top, for nearest-over-linear and linear-over-nearest.
    let red = rgba_image(2, 2, &[RED; 4]);
    let blue = rgba_image(2, 2, &[BLUE; 4]);
    for nearest_first in [true, false] {
        let (red, blue) = (red.clone(), blue.clone());
        let mut host = RenderHost::new();
        let rendered = host.render_logical(160.0, 160.0, move |_, _| {
            let (red, blue) = (red.clone(), blue.clone());
            painter(1.0, move |window| {
                let red_area = logical(20.0, 20.0, 80.0, 80.0);
                let blue_area = logical(60.0, 60.0, 80.0, 80.0);
                // The red image is painted nearest, the blue image linear.
                let paint_red = |window: &mut Window| {
                    window
                        .paint_image_nearest(
                            red_area,
                            red_area,
                            Corners::default(),
                            red.clone(),
                            0,
                            false,
                        )
                        .expect("nearest paints");
                };
                let paint_blue = |window: &mut Window| {
                    window
                        .paint_image(
                            blue_area,
                            blue_area,
                            Corners::default(),
                            blue.clone(),
                            0,
                            false,
                        )
                        .expect("linear paints");
                };
                if nearest_first {
                    paint_red(window);
                    paint_blue(window);
                } else {
                    paint_blue(window);
                    paint_red(window);
                }
            })
        });
        let overlap = pixel(&rendered, 160, 160);
        let expected = if nearest_first { rgb(BLUE) } else { rgb(RED) };
        assert!(
            close(overlap, expected, 2),
            "nearest_first = {nearest_first}: overlap {overlap:?}, expected {expected:?}"
        );
        assert!(close(pixel(&rendered, 60, 60), rgb(RED), 2));
        assert!(close(pixel(&rendered, 205, 205), rgb(BLUE), 2));
    }
}
