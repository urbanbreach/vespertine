//! Pixel tests for FID-THM-001, FID-THM-002 and THM-001 A8: swatches painted through an element
//! style color and through a component theme override field give identical pixels.

#[path = "../examples/example_support/swatches.rs"]
mod swatches;

use std::sync::Arc;

use swatches::{ColorPath, SwatchScene, THM_001_SWATCHES, composite, thm_002_swatches};
use vespertine::{AppContext as _, HeadlessAppContext, Rgba, WindowHandle, px, size};

fn render(scene: SwatchScene) -> image::RgbaImage {
    let window_size = size(px(scene.window.0), px(scene.window.1));
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(vespertine_wgpu::CosmicTextSystem::new("DejaVu Sans")),
        Arc::new(()),
        vespertine_platform::current_headless_renderer,
    );
    let window: WindowHandle<SwatchScene> = cx
        .open_window(window_size, |_, cx| cx.new(|_| scene))
        .expect("window opens");
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .expect("window draws");
    cx.capture_screenshot(window.into()).expect("screenshot")
}

/// The pixel at the logical point, from a screenshot rendered at the window's scale factor.
fn pixel(image: &image::RgbaImage, logical_width: f32, x: f32, y: f32) -> [u8; 3] {
    let scale = image.width() as f32 / logical_width;
    let pixel = image.get_pixel((x * scale) as u32, (y * scale) as u32);
    [pixel[0], pixel[1], pixel[2]]
}

fn assert_within(label: &str, actual: [u8; 3], expected: u32, tolerance: u8) {
    let expected = [
        (expected >> 16) as u8,
        (expected >> 8) as u8,
        expected as u8,
    ];
    for (a, e) in actual.into_iter().zip(expected) {
        assert!(
            a.abs_diff(e) <= tolerance,
            "{label}: {actual:02x?} vs #{expected:02x?}"
        );
    }
}

#[test]
fn fid_thm_001_swatches_match_expected_pixels_in_both_paths() {
    let style = render(SwatchScene::thm_001(ColorPath::ElementStyle));
    let themed = render(SwatchScene::thm_001(ColorPath::ThemeOverride));
    assert_eq!(style.dimensions(), themed.dimensions());
    assert_eq!(
        style.as_raw(),
        themed.as_raw(),
        "both color paths paint identical pixels"
    );

    for (index, (text, expected)) in THM_001_SWATCHES.iter().enumerate() {
        let (x, y) = swatches::thm_001_origin(index);
        assert_within(text, pixel(&style, 480., x + 20., y + 20.), *expected, 1);
    }
    // Everything outside the swatches stays white.
    assert_eq!(pixel(&style, 480., 5., 5.), [255, 255, 255]);
    assert_eq!(pixel(&style, 480., 475., 175.,), [255, 255, 255]);
    assert_eq!(pixel(&style, 480., 70., 50.), [255, 255, 255]);
}

#[test]
fn fid_thm_002_swatches_match_expected_pixels() {
    let image = render(SwatchScene::thm_002());
    let background = Rgba {
        r: 0x20 as f32 / 255.,
        g: 0x20 as f32 / 255.,
        b: 0x20 as f32 / 255.,
        a: 1.,
    };
    for (index, (fill, expected)) in thm_002_swatches().into_iter().enumerate() {
        let x = 20. + 70. * index as f32;
        assert_within(
            &format!("swatch {index}"),
            pixel(&image, 460., x + 20., 50.),
            expected,
            1,
        );
        assert_eq!(
            composite(fill, background),
            pixel(&image, 460., x + 20., 50.)
        );
    }
    assert_eq!(pixel(&image, 460., 5., 5.), [0x20, 0x20, 0x20]);
    assert_eq!(pixel(&image, 460., 65., 50.), [0x20, 0x20, 0x20]);
}
