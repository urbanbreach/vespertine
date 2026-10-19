//! LAY-005 A4: the FID-LAY-005 scene through the production painting path.
//!
//! Window 320 x 200 logical pixels at scale factor 2, opaque white. A gray 80 x 30 trigger at
//! (100, 60). A floating layer owned by the trigger, side bottom, alignment end, gap 6, snap
//! margin 8, priority 1, 100 x 50, fill #1A1A1A: attach point (180, 90), final bounds
//! (80, 96, 100, 50). Every other pixel stays white.

use crate::tests_host::RenderHost;
use image::{Rgba, RgbaImage};
use vespertine::{
    BoundsTracker, FloatingAlign, FloatingSide, IntoElement, ParentElement, Styled, div,
    floating_layer, px, rgb,
};

const SCALE: u32 = 2;

fn fill(image: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32, color: [u8; 3]) {
    for py in y * SCALE..(y + height) * SCALE {
        for px in x * SCALE..(x + width) * SCALE {
            image.put_pixel(px, py, Rgba([color[0], color[1], color[2], 255]));
        }
    }
}

/// Number of pixels differing by more than 8/255 in any channel.
fn differing_pixels(actual: &RgbaImage, expected: &RgbaImage) -> usize {
    actual
        .pixels()
        .zip(expected.pixels())
        .filter(|(a, e)| (0..3).any(|c| a.0[c].abs_diff(e.0[c]) > 8))
        .count()
}

#[test]
fn fid_lay_005_reproduces_the_fidelity_scene() {
    let tracker = BoundsTracker::new();
    let mut host = RenderHost::new();
    let image = host.render_logical(320.0, 200.0, {
        let tracker = tracker.clone();
        move |_, _| {
            div()
                .size_full()
                .bg(rgb(0xFFFFFF))
                .child(
                    div()
                        .absolute()
                        .left(px(100.))
                        .top(px(60.))
                        .w(px(80.))
                        .h(px(30.))
                        .bg(rgb(0x808080))
                        .child(
                            floating_layer("fid-lay-005-layer")
                                .side(FloatingSide::Bottom)
                                .align(FloatingAlign::End)
                                .gap(px(6.))
                                .snap_margin(px(8.))
                                .priority(1)
                                .bounds_tracker(&tracker)
                                .w(px(100.))
                                .h(px(50.))
                                .bg(rgb(0x1A1A1A)),
                        ),
                )
                .into_any_element()
        }
    });

    let bounds = tracker.get().expect("layer painted");
    assert_eq!(
        (
            f32::from(bounds.origin.x),
            f32::from(bounds.origin.y),
            f32::from(bounds.size.width),
            f32::from(bounds.size.height)
        ),
        (80., 96., 100., 50.)
    );

    assert_eq!(image.dimensions(), (320 * SCALE, 200 * SCALE));
    let mut expected = RgbaImage::from_pixel(320 * SCALE, 200 * SCALE, Rgba([255, 255, 255, 255]));
    fill(&mut expected, 100, 60, 80, 30, [0x80, 0x80, 0x80]);
    fill(&mut expected, 80, 96, 100, 50, [0x1A, 0x1A, 0x1A]);
    let differing = differing_pixels(&image, &expected);
    assert!(
        differing * 1000 <= image.width() as usize * image.height() as usize * 5,
        "{differing} pixels differ by more than 8/255 (tolerance 0.5%)"
    );
    // The scene is axis-aligned at integer coordinates, so it is in fact exact.
    assert_eq!(differing, 0);
}
