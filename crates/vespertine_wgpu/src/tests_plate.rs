//! RND-008 (layered surface painting) pixel acceptance tests through the production
//! painting path: the styled plate and the canvas-painted plate of FID-RND-008 must match,
//! inset shadows light exactly the top row, translucent fills show their drop shadow and the
//! backdrop in proportion to `1 - alpha`.

#[path = "../../vespertine/examples/example_support/surface_plate.rs"]
mod surface_plate;

use crate::tests_host::RenderHost;
use image::RgbaImage;
use surface_plate::{canvas_plate, scene_root as root, styled_plate};
use vespertine::{
    BoxShadow, Hsla, IntoElement, ParentElement, Pixels, Styled, black, div, point, px, rgb, white,
};

/// Fraction of pixels in the compared regions that differ by more than 8/255 in any channel.
fn differing_fraction(
    a: &RgbaImage,
    b: &RgbaImage,
    a_origin: (u32, u32),
    b_origin: (u32, u32),
    size: (u32, u32),
) -> f32 {
    let mut differing = 0usize;
    for y in 0..size.1 {
        for x in 0..size.0 {
            let pa = a.get_pixel(a_origin.0 + x, a_origin.1 + y).0;
            let pb = b.get_pixel(b_origin.0 + x, b_origin.1 + y).0;
            if pa.iter().zip(pb).any(|(pa, pb)| pa.abs_diff(pb) > 8) {
                differing += 1;
            }
        }
    }
    differing as f32 / (size.0 * size.1) as f32
}

#[test]
fn styled_plate_and_canvas_plate_match() {
    let mut host = RenderHost::new();
    let image = host.render_logical(800., 480., |_, _| {
        root()
            .child(styled_plate(80., 80.))
            .child(canvas_plate(420., 80.))
            .into_any_element()
    });
    // Compare logical x in [40, 360) against [380, 700): 320 x 200 logical pixels each side
    // of the plates, shadow margins included.
    let fraction = differing_fraction(&image, &image, (80, 100), (760, 100), (640, 400));
    assert!(
        fraction <= 0.005,
        "styled and canvas plates differ in {:.3}% of pixels",
        fraction * 100.
    );
    // The plate is actually painted: a pixel inside differs from the background.
    let inside = image.get_pixel(2 * 200, 2 * 100).0;
    assert!(inside[0].abs_diff(0x18) > 20);
}

fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
    image.get_pixel(x, y).0
}

#[test]
fn inset_shadow_offset_one_lights_exactly_the_top_logical_row() {
    let inset = BoxShadow {
        color: white(),
        offset: point(px(0.), px(1.)),
        blur_radius: px(0.),
        spread_radius: px(0.),
        inset: true,
    };
    let mut host = RenderHost::new();
    let image = host.render_logical(60., 40., move |_, _| {
        let element = |radius: Pixels| {
            div()
                .absolute()
                .top(px(10.))
                .w(px(20.))
                .h(px(20.))
                .rounded(radius)
                .bg(black())
                .shadow(vec![inset.clone()])
        };
        root()
            .child(element(px(0.)).left(px(5.)))
            .child(element(px(8.)).left(px(35.)))
            .into_any_element()
    });
    // Unrounded element at x = 5..25, y = 10..30 logical: device rows 20 and 21 are lit.
    let x = 2 * 15;
    assert_eq!(
        pixel(&image, x, 19)[0],
        0x18,
        "row above the element is background"
    );
    assert_eq!(pixel(&image, x, 20), [255, 255, 255, 255]);
    assert_eq!(pixel(&image, x, 21), [255, 255, 255, 255]);
    assert_eq!(pixel(&image, x, 22)[..3], [0, 0, 0], "row 3 is the fill");
    assert_eq!(pixel(&image, x, 30)[..3], [0, 0, 0]);
    // Rounded element: the lit rows are clipped to the rounded bounds at the corners.
    assert_eq!(pixel(&image, 2 * 35 + 1, 20)[..3], [0x18, 0x1A, 0x1F]);
    assert_eq!(pixel(&image, 2 * 45, 20), [255, 255, 255, 255]);
}

#[test]
fn translucent_fill_shows_drop_shadow_and_backdrop_in_proportion_to_alpha() {
    let shadow = BoxShadow {
        color: black(),
        offset: point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(4.),
        inset: false,
    };
    let mut host = RenderHost::new();
    let image = host.render_logical(120., 60., move |_, _| {
        let translucent = |left: f32| {
            div()
                .absolute()
                .left(px(left))
                .top(px(10.))
                .w(px(30.))
                .h(px(30.))
                .bg(Hsla { a: 0.5, ..white() })
        };
        root()
            .child(
                div()
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .w(px(120.))
                    .h(px(60.))
                    .bg(rgb(0xff0000)),
            )
            .child(translucent(10.))
            .child(translucent(70.).shadow(vec![shadow.clone()]))
            .into_any_element()
    });
    // Without a shadow: 50% white over red keeps the backdrop in proportion (1 - alpha).
    let plain = pixel(&image, 2 * 25, 2 * 25);
    assert!(plain[0] >= 253 && plain[1].abs_diff(128) <= 2 && plain[2].abs_diff(128) <= 2);
    // With a drop shadow: the shadow is not cut out beneath the box, so black shows
    // through the translucent fill.
    let shadowed = pixel(&image, 2 * 85, 2 * 25);
    assert!(shadowed[0].abs_diff(128) <= 2 && shadowed[1].abs_diff(128) <= 2);
    assert!(shadowed[2].abs_diff(128) <= 2);
}
