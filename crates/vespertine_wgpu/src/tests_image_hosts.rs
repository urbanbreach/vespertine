//! Production-path pixel tests of the media hosts (ELM-004) and of fitted image elements
//! (ELM-003): channel order and straight alpha through the real renderer, and the geometry of
//! FID-ELM-003.

use crate::tests_host::RenderHost;
use image::RgbaImage;
use std::{cell::RefCell, rc::Rc, sync::Arc};
use vespertine::{
    AnyElement, ImageHostState, ImageSource, IntoElement, ObjectFit, ParentElement, PixelFormat,
    StyleRefinement, Styled, StyledImage, div, img, px, render_image_from_pixels, rgb,
};

fn pixel(image: &RgbaImage, x: u32, y: u32) -> [u8; 3] {
    let p = image.get_pixel(x, y).0;
    [p[0], p[1], p[2]]
}

fn close(actual: [u8; 3], expected: [u8; 3], tolerance: u8) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(e) <= tolerance)
}

/// Renders a live image host (a 40 x 40 box on a #204060 background) holding a 4 x 4 image of
/// one pixel value in the given layout.
fn render_host(format: &str, texel: [u8; 4]) -> RgbaImage {
    let mut pixels = Vec::new();
    for _ in 0..16 {
        pixels.extend_from_slice(&texel);
    }
    let mut state = ImageHostState::new();
    state
        .set_image_pixels(4., 4., &pixels, Some(format))
        .expect("valid pixels");
    let state = Rc::new(RefCell::new(state));
    let mut host = RenderHost::new();
    host.render_logical(60., 60., move |window, cx| {
        let style = StyleRefinement::default().w(px(40.)).h(px(40.));
        let element = state.borrow_mut().element("host", style, window, cx);
        div()
            .size_full()
            .bg(rgb(0x204060))
            .child(element)
            .into_any_element()
    })
}

#[test]
fn rgba_red_and_bgra_red_render_the_same_red() {
    let rgba = render_host("rgba", [255, 0, 0, 255]);
    let bgra = render_host("bgra", [0, 0, 255, 255]);
    assert!(
        close(pixel(&rgba, 40, 40), [255, 0, 0], 2),
        "{:?}",
        pixel(&rgba, 40, 40)
    );
    assert!(
        close(pixel(&bgra, 40, 40), [255, 0, 0], 2),
        "{:?}",
        pixel(&bgra, 40, 40)
    );
    assert_eq!(pixel(&rgba, 40, 40), pixel(&bgra, 40, 40));
    // Outside the 40 x 40 box the background shows.
    assert!(close(pixel(&rgba, 110, 110), [0x20, 0x40, 0x60], 2));
}

#[test]
fn half_alpha_stays_half_alpha_over_the_background_for_both_formats() {
    // 50% red over #204060, straight alpha: 0.502 * red + 0.498 * background.
    let expected = [
        (255.0 * 128.0 / 255.0 + 0x20 as f32 * 127.0 / 255.0).round() as u8,
        (0x40 as f32 * 127.0 / 255.0).round() as u8,
        (0x60 as f32 * 127.0 / 255.0).round() as u8,
    ];
    let rgba = render_host("rgba", [255, 0, 0, 128]);
    let bgra = render_host("bgra", [0, 0, 255, 128]);
    assert!(
        close(pixel(&rgba, 40, 40), expected, 3),
        "{:?} vs {expected:?}",
        pixel(&rgba, 40, 40)
    );
    assert!(
        close(pixel(&bgra, 40, 40), expected, 3),
        "{:?} vs {expected:?}",
        pixel(&bgra, 40, 40)
    );
    assert_eq!(pixel(&rgba, 40, 40), pixel(&bgra, 40, 40));

    // The polychrome path (an SVG through the image element) keeps its alpha the same way.
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="#FF0000" fill-opacity="0.5"/></svg>"##;
    let mut host = RenderHost::new();
    let rendered = host.render_logical(60., 60., move |_, cx| {
        // Decoded up front: asset loading is asynchronous and the host draws one frame.
        let image = vespertine::decode_image_bytes(svg.as_bytes(), &cx.svg_renderer())
            .expect("svg decodes");
        div()
            .size_full()
            .bg(rgb(0x204060))
            .child(img(ImageSource::Render(image)).w(px(40.)).h(px(40.)))
            .into_any_element()
    });
    assert!(
        close(pixel(&rendered, 40, 40), expected, 4),
        "{:?} vs {expected:?}",
        pixel(&rendered, 40, 40)
    );
}

fn stripes() -> Arc<vespertine::RenderImage> {
    let mut pixels = Vec::new();
    for _ in 0..160 {
        for x in 0..320usize {
            let color: [u8; 4] = if (x / 40) % 2 == 0 {
                [255, 255, 255, 255]
            } else {
                [0, 0x80, 255, 255]
            };
            pixels.extend_from_slice(&color);
        }
    }
    Arc::new(
        render_image_from_pixels(320., 160., &pixels, PixelFormat::Rgba).expect("valid pixels"),
    )
}

fn fid_elm_003() -> RgbaImage {
    let image = stripes();
    let mut host = RenderHost::new();
    host.render_logical(400., 220., move |_, _| -> AnyElement {
        let thumbnail = |fit: ObjectFit, x: f32| {
            div().absolute().left(px(x)).top(px(20.)).child(
                img(ImageSource::Render(image.clone()))
                    .w(px(160.))
                    .h(px(160.))
                    .rounded(px(24.))
                    .object_fit(fit),
            )
        };
        div()
            .relative()
            .size_full()
            .bg(rgb(0x202020))
            .child(thumbnail(ObjectFit::Cover, 20.))
            .child(thumbnail(ObjectFit::Contain, 220.))
            .into_any_element()
    })
}

#[test]
fn fid_elm_003_cover_crops_four_stripes_with_rounded_visible_corners() {
    const WHITE: [u8; 3] = [255, 255, 255];
    const BLUE: [u8; 3] = [0, 0x80, 255];
    const BACKGROUND: [u8; 3] = [0x20, 0x20, 0x20];
    let image = fid_elm_003();
    assert_eq!(image.dimensions(), (800, 440));
    // Visible source columns 80..240: stripes (logical x) 20..60 white, 60..100 blue,
    // 100..140 white, 140..180 blue; device pixels are twice that.
    for (logical_x, expected) in [(40, WHITE), (80, BLUE), (120, WHITE), (160, BLUE)] {
        assert!(
            close(pixel(&image, logical_x * 2, 200), expected, 3),
            "x {logical_x}: {:?}",
            pixel(&image, logical_x * 2, 200)
        );
    }
    // All four visible corners are rounded (radius 24 logical = 48 device).
    for (x, y) in [(41, 41), (358, 41), (41, 358), (358, 358)] {
        assert!(close(pixel(&image, x, y), BACKGROUND, 3), "corner {x},{y}");
    }
    // Just inside the straight edges the image shows.
    assert!(
        close(pixel(&image, 150, 42), BLUE, 3),
        "{:?}",
        pixel(&image, 150, 42)
    );
    // Outside the element the background stays.
    assert!(close(pixel(&image, 380, 200), BACKGROUND, 3));
}

#[test]
fn fid_elm_003_contain_is_a_letterboxed_rectangle_with_its_own_rounded_corners() {
    const WHITE: [u8; 3] = [255, 255, 255];
    const BLUE: [u8; 3] = [0, 0x80, 255];
    const BACKGROUND: [u8; 3] = [0x20, 0x20, 0x20];
    let image = fid_elm_003();
    // The image is 160 x 80 logical at (220,60): device x 440..760, y 120..280. Stripes are
    // 20 logical wide: 220..240 white, 240..260 blue.
    assert!(close(pixel(&image, 2 * 230, 400 / 2), WHITE, 3));
    assert!(close(pixel(&image, 2 * 250, 400 / 2), BLUE, 3));
    // The letterbox bands above and below show the background, not the image.
    assert!(close(pixel(&image, 2 * 230, 2 * 40), BACKGROUND, 3));
    assert!(close(pixel(&image, 2 * 230, 2 * 150), BACKGROUND, 3));
    // The corners of the image rectangle itself are rounded.
    for (x, y) in [(441, 121), (758, 121), (441, 278), (758, 278)] {
        assert!(close(pixel(&image, x, y), BACKGROUND, 3), "corner {x},{y}");
    }
    // Straight edges between the corners are filled.
    assert!(!close(pixel(&image, 600, 122), BACKGROUND, 3));
}
