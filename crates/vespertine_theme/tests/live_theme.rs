//! THM-002 A5: a theme value change reaches the next frame (colors and row positions).

#[path = "../examples/example_support/code_rows.rs"]
mod code_rows;

use std::sync::Arc;

use code_rows::{CodeRows, steps};
use vespertine::{AppContext as _, HeadlessAppContext, WindowHandle, px, size};
use vespertine_theme::document::DocumentTheme;

fn frame(cx: &mut HeadlessAppContext, window: WindowHandle<CodeRows>) -> image::RgbaImage {
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .expect("draw");
    cx.capture_screenshot(window.into()).expect("screenshot")
}

fn rgb_at(image: &image::RgbaImage, x: f32, y: f32) -> [u8; 3] {
    let scale = image.width() as f32 / 300.;
    let pixel = image.get_pixel((x * scale) as u32, (y * scale) as u32);
    [pixel[0], pixel[1], pixel[2]]
}

fn near(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual
        .into_iter()
        .zip(expected)
        .all(|(a, e)| a.abs_diff(e) <= 1)
}

#[test]
fn text_color_and_line_height_change_at_runtime() {
    let mut cx = HeadlessAppContext::with_platform(
        Arc::new(vespertine_wgpu::CosmicTextSystem::new("DejaVu Sans")),
        Arc::new(()),
        vespertine_platform::current_headless_renderer,
    );
    let window: WindowHandle<CodeRows> = cx
        .open_window(size(px(300.), px(300.)), |_, cx| {
            cx.new(|_| CodeRows::new(DocumentTheme::dark()))
        })
        .unwrap();
    let rows = window.entity(&cx).unwrap();

    // Defaults: text #E5E5E5 chips, 18px rows whose last pixel line is the accent separator.
    let before = frame(&mut cx, window);
    let accent = [0x7C, 0x86, 0xFF];
    assert!(near(rgb_at(&before, 3., 9.), [0xE5, 0xE5, 0xE5]));
    assert!(
        near(rgb_at(&before, 3., 17.5), accent),
        "row 0 separator at y=17"
    );
    assert!(
        near(rgb_at(&before, 3., 35.5), accent),
        "row 1 separator at y=35"
    );
    assert!(!near(rgb_at(&before, 3., 29.), accent));

    // Runtime override of the text color and line height through the theme value.
    let (_, value) = steps().remove(1);
    cx.update(|app| {
        rows.update(app, |rows, cx| {
            rows.set_theme(DocumentTheme::from_value(Some(&value)), cx)
        })
    });
    let after = frame(&mut cx, window);
    assert!(
        near(rgb_at(&after, 3., 15.), [0xFF, 0xCC, 0x00]),
        "new text color"
    );
    assert!(
        near(rgb_at(&after, 3., 29.5), [0xFF, 0x3B, 0x30]),
        "row 0 separator at y=29"
    );
    assert!(
        near(rgb_at(&after, 3., 59.5), [0xFF, 0x3B, 0x30]),
        "row 1 separator at y=59"
    );
    assert!(
        !near(rgb_at(&after, 3., 17.5), [0xFF, 0x3B, 0x30]),
        "old separator is gone"
    );

    // Light appearance with 24px rows.
    let (_, value) = steps().remove(2);
    cx.update(|app| {
        rows.update(app, |rows, cx| {
            rows.set_theme(DocumentTheme::from_value(Some(&value)), cx)
        })
    });
    let light = frame(&mut cx, window);
    assert!(near(rgb_at(&light, 3., 12.), [0x22, 0x22, 0x22]));
    assert!(
        near(rgb_at(&light, 100., 280.), [0xFF, 0xFF, 0xFF]),
        "light background"
    );
    assert!(near(rgb_at(&light, 3., 23.5), [0x4F, 0x39, 0xF6]));
}
