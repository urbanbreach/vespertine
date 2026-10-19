//! Pixel tests of fitted image painting and the feathered inverse rounded-rectangle mask
//! (RND-004) through the production path: `Window::paint_image*`, the scene, the wgpu
//! renderer and its shaders, compared with the reference arithmetic in `vespertine`.

use crate::tests_host::RenderHost;
use image::RgbaImage;
use smallvec::SmallVec;
use std::sync::Arc;
use vespertine::{
    AnyElement, Bounds, ContentMask, Corners, EdgeFade, ImageAlphaMask, IntoElement as _,
    ParentElement as _, Pixels, RenderImage, ScopeParams, Styled as _, Window, canvas, div, point,
    px, rgb, size,
};

pub(crate) fn logical(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

/// An element that fills the window with opaque black and runs `paint` in the paint phase,
/// under an element opacity of `opacity`.
pub(crate) fn painter(opacity: f32, paint: impl Fn(&mut Window) + 'static) -> AnyElement {
    let paint = Arc::new(paint);
    div()
        .size_full()
        .bg(rgb(0x000000))
        .child(
            div()
                .size_full()
                .opacity(opacity)
                .child(canvas(|_, _, _| (), move |_, _, window, _| paint(window)).size_full()),
        )
        .into_any_element()
}

/// A single-frame image from straight-alpha RGBA texels, row-major.
pub(crate) fn rgba_image(width: u32, height: u32, texels: &[[u8; 4]]) -> Arc<RenderImage> {
    Arc::new(RenderImage::from_bgra(width, height, bgra_bytes(texels)).expect("valid image"))
}

fn bgra_bytes(texels: &[[u8; 4]]) -> Vec<u8> {
    texels
        .iter()
        .flat_map(|[r, g, b, a]| [*b, *g, *r, *a])
        .collect()
}

/// A multi-frame image whose frames are `rgba_image`-style texel lists of equal size.
pub(crate) fn multi_frame_image(
    width: u32,
    height: u32,
    frames: &[&[[u8; 4]]],
) -> Arc<RenderImage> {
    let frames: SmallVec<[image::Frame; 1]> = frames
        .iter()
        .map(|texels| {
            let buffer = RgbaImage::from_raw(width, height, bgra_bytes(texels)).expect("frame");
            image::Frame::new(buffer)
        })
        .collect();
    Arc::new(RenderImage::new(frames))
}

fn channel(image: &RgbaImage, x: u32, y: u32, channel: usize) -> f32 {
    f32::from(image.get_pixel(x, y).0[channel]) / 255.0
}

/// Signed distance to a rounded rectangle (device pixels), as the shader's `quad_sdf`.
fn rounded_rect_sdf(p: [f32; 2], x: f32, y: f32, w: f32, h: f32, radius: f32) -> f32 {
    let half = [w / 2.0, h / 2.0];
    let q = [
        (p[0] - (x + half[0])).abs() - half[0] + radius,
        (p[1] - (y + half[1])).abs() - half[1] + radius,
    ];
    (q[0].max(0.0).powi(2) + q[1].max(0.0).powi(2)).sqrt() + q[0].max(q[1]).min(0.0) - radius
}

const WHITE: [u8; 4] = [255, 255, 255, 255];

/// A solid white image of 32 x 32 texels. The linear sampler of the shared atlas does not clamp
/// at the tile edge (RND-007), so the outer half texel of any image blends with the empty
/// atlas around the tile; the tests compare regions inside that ring (two device pixels at
/// the sizes used here).
fn white_image() -> Arc<RenderImage> {
    rgba_image(32, 32, &[WHITE; 32 * 32])
}

fn assert_matches(
    image: &RgbaImage,
    region: (u32, u32, u32, u32),
    expected: impl Fn(f32, f32) -> f32,
    tolerance: f32,
    what: &str,
) {
    let (x0, y0, x1, y1) = region;
    for y in y0..y1 {
        for x in x0..x1 {
            let want = expected(x as f32 + 0.5, y as f32 + 0.5);
            let got = channel(image, x, y, 0);
            assert!(
                (got - want).abs() <= tolerance,
                "{what}: ({x}, {y}) rendered {got}, expected {want}"
            );
        }
    }
}

#[test]
fn masked_image_matches_reference_on_every_pixel() {
    // Solid white image over black: the red channel is the mask coverage.
    let mask = ImageAlphaMask {
        bounds: logical(20.0, 20.0, 24.0, 20.0),
        radius: px(6.0),
        feather: px(10.0),
        clearance: px(3.0),
        bottom_fade: Some((px(56.0), px(20.0))),
    };
    let image = white_image();
    let mut host = RenderHost::new();
    let rendered = host.render_logical(64.0, 64.0, move |_, _| {
        let image = image.clone();
        painter(1.0, move |window| {
            let area = logical(0.0, 0.0, 64.0, 64.0);
            window
                .paint_image_fitted_masked(
                    area,
                    area,
                    Corners::default(),
                    image.clone(),
                    0,
                    false,
                    Some(mask),
                )
                .expect("paints");
        })
    });
    assert_eq!(rendered.dimensions(), (128, 128));
    let effects = mask.to_effects(2.0);
    assert_matches(
        &rendered,
        (3, 3, 125, 125),
        |x, y| effects.mask_coverage([x, y]),
        3.0 / 255.0,
        "mask",
    );
    // Landmarks: the cutout is gone, the far corner untouched, the bottom ramp fades out.
    assert!(channel(&rendered, 64, 60, 0) < 0.01);
    assert!(channel(&rendered, 4, 4, 0) > 0.99);
    assert!(channel(&rendered, 4, 127, 0) < 0.01);
}

#[test]
fn two_masks_over_one_texture_each_follow_their_own_geometry() {
    // RND-004 R7: the same image drawn twice with different mask parameters.
    let first = ImageAlphaMask {
        bounds: logical(10.0, 10.0, 20.0, 20.0),
        radius: px(4.0),
        feather: px(8.0),
        clearance: px(0.0),
        bottom_fade: None,
    };
    let second = ImageAlphaMask {
        bounds: logical(84.0, 20.0, 30.0, 30.0),
        radius: px(15.0),
        feather: px(16.0),
        clearance: px(4.0),
        bottom_fade: Some((px(60.0), px(30.0))),
    };
    let image = white_image();
    let mut host = RenderHost::new();
    let rendered = host.render_logical(128.0, 64.0, move |_, _| {
        let image = image.clone();
        painter(1.0, move |window| {
            for (x, mask) in [(0.0, first), (64.0, second)] {
                let area = logical(x, 0.0, 64.0, 64.0);
                window
                    .paint_image_fitted_masked(
                        area,
                        area,
                        Corners::default(),
                        image.clone(),
                        0,
                        false,
                        Some(mask),
                    )
                    .expect("paints");
            }
        })
    });
    for (mask, region) in [(first, (3, 3, 125, 125)), (second, (131, 3, 253, 125))] {
        let effects = mask.to_effects(2.0);
        assert_matches(
            &rendered,
            region,
            |x, y| effects.mask_coverage([x, y]),
            3.0 / 255.0,
            "mask",
        );
    }
}

fn checkerboard() -> Arc<RenderImage> {
    let mut texels = Vec::new();
    for y in 0..8u8 {
        for x in 0..8u8 {
            texels.push(if (x + y) % 2 == 0 {
                [0x70, 0xB8, 0xFF, 255]
            } else {
                [0xFF, 0xC8, 0x78, 255]
            });
        }
    }
    rgba_image(8, 8, &texels)
}

#[test]
fn zero_feather_and_absent_mask_render_exactly_like_ordinary_painting() {
    let area = logical(4.0, 4.0, 56.0, 56.0);
    let render = |paint: Arc<dyn Fn(&mut Window, Arc<RenderImage>)>| {
        let image = checkerboard();
        let mut host = RenderHost::new();
        host.render_logical(64.0, 64.0, move |_, _| {
            let image = image.clone();
            let paint = paint.clone();
            painter(0.9, move |window| paint(window, image.clone()))
        })
    };
    let ordinary = render(Arc::new(move |window, image| {
        window
            .paint_image(area, area, Corners::all(px(10.0)), image, 0, false)
            .expect("paints");
    }));
    let fitted = render(Arc::new(move |window, image| {
        window
            .paint_image_fitted(area, area, Corners::all(px(10.0)), image, 0, false)
            .expect("paints");
    }));
    let absent = render(Arc::new(move |window, image| {
        window
            .paint_image_fitted_masked(area, area, Corners::all(px(10.0)), image, 0, false, None)
            .expect("paints");
    }));
    // A zero feather disables the mask even with a positive bottom fade.
    let zero_feather = ImageAlphaMask {
        bounds: logical(20.0, 20.0, 24.0, 24.0),
        radius: px(6.0),
        feather: px(0.0),
        clearance: px(4.0),
        bottom_fade: Some((px(40.0), px(30.0))),
    };
    let zero = render(Arc::new(move |window, image| {
        window
            .paint_image_fitted_masked(
                area,
                area,
                Corners::all(px(10.0)),
                image,
                0,
                false,
                Some(zero_feather),
            )
            .expect("paints");
    }));
    // A negative feather is clamped to zero by the descriptor.
    let negative = ImageAlphaMask {
        feather: px(-5.0),
        ..zero_feather
    };
    let negative = render(Arc::new(move |window, image| {
        window
            .paint_image_fitted_masked(
                area,
                area,
                Corners::all(px(10.0)),
                image,
                0,
                false,
                Some(negative),
            )
            .expect("paints");
    }));
    assert_eq!(ordinary.as_raw(), fitted.as_raw());
    assert_eq!(ordinary.as_raw(), absent.as_raw());
    assert_eq!(ordinary.as_raw(), zero.as_raw());
    assert_eq!(ordinary.as_raw(), negative.as_raw());
    // The image is really there.
    assert!(channel(&ordinary, 64, 64, 0) > 0.3);
}

#[test]
fn mask_composes_with_texture_alpha_corners_edge_fade_and_opacity() {
    // RND-004 A6. A 2 x 2 white image with straight alpha 64 / 255 over 255 / 64, painted
    // nearest-free (linear): between the texel centers the texture alpha is the bilinear
    // blend. Visible (8,8) 64 x 64 at scale 2 -> device
    // (16,16) 128 x 128, corner radius 12 logical, top edge fade of band 16 logical,
    // element opacity 0.6, mask (30,30) 20 x 20.
    let texels = [
        [255, 255, 255, 64],
        [255, 255, 255, 255],
        [255, 255, 255, 255],
        [255, 255, 255, 64],
    ];
    let image = rgba_image(2, 2, &texels);
    let mask = ImageAlphaMask {
        bounds: logical(30.0, 30.0, 20.0, 20.0),
        radius: px(5.0),
        feather: px(12.0),
        clearance: px(2.0),
        bottom_fade: None,
    };
    let visible = logical(8.0, 8.0, 64.0, 64.0);
    let mut fade = EdgeFade::new(visible, px(16.0));
    fade.top = true;
    let mut host = RenderHost::new();
    let rendered = host.render_logical(80.0, 80.0, move |_, _| {
        let image = image.clone();
        painter(0.6, move |window| {
            let image = image.clone();
            window.with_edge_fade(Some(fade), |window| {
                window
                    .paint_image_fitted_masked(
                        visible,
                        visible,
                        Corners::all(px(12.0)),
                        image,
                        0,
                        false,
                        Some(mask),
                    )
                    .expect("paints");
            });
        })
    });
    let scope = ScopeParams {
        fade_edges: [16.0, 0.0, 0.0, 0.0],
        fade_bands: [32.0, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    let effects = mask.to_effects(2.0);
    // Texture alpha of the stretched 2 x 2 image: the flat top-left corner (within half a
    // texel of the clamped left and top edges) and the interior between texel centers,
    // which is the bilinear blend of the four texels. The right and bottom half texels are
    // not compared: they blend with the neighboring atlas texels (atlas edge handling is not
    // part of this capability).
    let alpha_of = |x: f32, y: f32| {
        let u = ((x - 48.0) / 64.0).clamp(0.0, 1.0);
        let v = ((y - 48.0) / 64.0).clamp(0.0, 1.0);
        let top = 64.0 + (255.0 - 64.0) * u;
        let bottom = 255.0 + (64.0 - 255.0) * u;
        (top + (bottom - top) * v) / 255.0
    };
    // The flat corner quadrants are not compared: the linear sampler blends the outer half
    // texel with the empty atlas next to the tile (RND-007), so only the interior holds.
    assert_matches(
        &rendered,
        (49, 49, 111, 111),
        |x, y| {
            let aa =
                (0.5 - rounded_rect_sdf([x, y], 16.0, 16.0, 128.0, 128.0, 24.0)).clamp(0.0, 1.0);
            alpha_of(x, y) * 0.6 * aa * scope.factor([x, y]) * effects.mask_coverage([x, y])
        },
        4.0 / 255.0,
        "composition",
    );
    // No RGB fringe: red, green and blue agree everywhere (white content scaled by alpha).
    for y in 0..160 {
        for x in 0..160 {
            let p = rendered.get_pixel(x, y).0;
            assert!(
                p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1,
                "({x}, {y}): {p:?}"
            );
        }
    }
}

#[test]
fn mask_composes_with_rounded_scope_and_content_mask() {
    let mask = ImageAlphaMask {
        bounds: logical(28.0, 28.0, 16.0, 16.0),
        radius: px(8.0),
        feather: px(10.0),
        clearance: px(0.0),
        bottom_fade: None,
    };
    let clip = logical(8.0, 8.0, 48.0, 48.0);
    let content = logical(0.0, 0.0, 40.0, 64.0);
    let image = white_image();
    let mut host = RenderHost::new();
    let rendered = host.render_logical(64.0, 64.0, move |_, _| {
        let image = image.clone();
        painter(1.0, move |window| {
            let image = image.clone();
            window.with_content_mask(Some(ContentMask { bounds: content }), |window| {
                window.with_rounded_clip(clip, px(16.0), |window| {
                    let area = logical(0.0, 0.0, 64.0, 64.0);
                    window
                        .paint_image_fitted_masked(
                            area,
                            area,
                            Corners::default(),
                            image,
                            0,
                            false,
                            Some(mask),
                        )
                        .expect("paints");
                });
            });
        })
    });
    let scope = ScopeParams {
        clip_bounds: [16.0, 16.0, 112.0, 112.0],
        clip_radius: 32.0,
        ..Default::default()
    };
    let effects = mask.to_effects(2.0);
    // Left of the rectangular content mask edge (x = 80 device) the product holds; right of
    // it nothing is painted. Skip the one-pixel band at the mask edge.
    assert_matches(
        &rendered,
        (0, 0, 78, 128),
        |x, y| scope.factor([x, y]) * effects.mask_coverage([x, y]),
        4.0 / 255.0,
        "rounded scope",
    );
    assert_matches(
        &rendered,
        (82, 0, 128, 128),
        |_, _| 0.0,
        1.0 / 255.0,
        "content mask",
    );
}

#[test]
fn grayscale_and_frame_selection_behave_identically_with_and_without_masks() {
    // Two 2 x 2 frames of flat color; frame 1 is the one painted.
    let frame0 = [[200, 100, 50, 255]; 4];
    let frame1 = [[50, 100, 200, 255]; 4];
    let image = multi_frame_image(2, 2, &[&frame0, &frame1]);
    let mask = ImageAlphaMask {
        bounds: logical(24.0, 24.0, 16.0, 16.0),
        radius: px(4.0),
        feather: px(8.0),
        clearance: px(0.0),
        bottom_fade: None,
    };
    let area = logical(0.0, 0.0, 64.0, 64.0);
    let mut host = RenderHost::new();
    let mut render = |frame: usize, grayscale: bool, mask: Option<ImageAlphaMask>| {
        let image = image.clone();
        host.render_logical(64.0, 64.0, move |_, _| {
            let image = image.clone();
            painter(1.0, move |window| {
                window
                    .paint_image_fitted_masked(
                        area,
                        area,
                        Corners::default(),
                        image.clone(),
                        frame,
                        grayscale,
                        mask,
                    )
                    .expect("paints");
            })
        })
    };
    let color_plain = render(1, false, None);
    let color_masked = render(1, false, Some(mask));
    let gray_plain = render(1, true, None);
    let gray_masked = render(1, true, Some(mask));
    let first_frame = render(0, false, None);

    // Frame selection: frame 1 is blue-ish, frame 0 red-ish.
    let pixel = |image: &RgbaImage| image.get_pixel(8, 8).0;
    assert!(pixel(&color_plain)[2] > pixel(&color_plain)[0]);
    assert!(pixel(&first_frame)[0] > pixel(&first_frame)[2]);
    // Far from the cutout the mask changes nothing, in color and in grayscale.
    assert_eq!(pixel(&color_plain), pixel(&color_masked));
    assert_eq!(pixel(&gray_plain), pixel(&gray_masked));
    let gray = pixel(&gray_plain);
    assert!(gray[0].abs_diff(gray[1]) <= 1 && gray[1].abs_diff(gray[2]) <= 1);
    assert!(gray != pixel(&color_plain));
    // Inside the feather band the grayscale result keeps the alpha: it is the plain
    // grayscale pixel scaled by the mask coverage, exactly like the color pixel.
    let effects = mask.to_effects(2.0);
    for (x, y) in [(88u32, 64u32), (64, 88), (64, 40), (40, 64), (84, 84)] {
        let coverage = effects.mask_coverage([x as f32 + 0.5, y as f32 + 0.5]);
        assert!(
            coverage > 0.05 && coverage < 0.95,
            "probe ({x}, {y}) coverage {coverage}"
        );
        for (plain, masked) in [(&gray_plain, &gray_masked), (&color_plain, &color_masked)] {
            for channel_index in 0..3 {
                let want = channel(plain, x, y, channel_index) * coverage;
                let got = channel(masked, x, y, channel_index);
                assert!(
                    (got - want).abs() <= 3.0 / 255.0,
                    "({x}, {y}) {got} vs {want}"
                );
            }
        }
    }
}
