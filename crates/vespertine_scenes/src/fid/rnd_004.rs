//! FID-RND-004-1/2/3: fitted image painting with the feathered inverse rounded-rectangle mask
//! (magnified checkerboard with bottom ramp, fitted crop with intact corners, cover-fit with a
//! tall cutout reaching the image bottom).

use std::sync::Arc;

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Bounds, Corners, ImageAlphaMask, IntoElement, ParentElement, Pixels,
    RenderImage, Styled, Window, canvas, div, point, px, size,
};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

/// An opaque image whose pixel `(x, y)` has the sRGB hex color `color(x, y)`, stored as BGRA.
fn generated_image(width: u32, height: u32, color: impl Fn(u32, u32) -> u32) -> Arc<RenderImage> {
    let mut bytes = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let value = color(x, y);
            bytes.extend_from_slice(&[value as u8, (value >> 8) as u8, (value >> 16) as u8, 255]);
        }
    }
    Arc::new(RenderImage::from_bgra(width, height, bytes).expect("valid generated image"))
}

/// A full-window element with `background` holding one full-window canvas that runs `paint`.
/// With an `opacity`, the canvas sits in an inner full-window element of that element opacity
/// (the background of the outer element is not dimmed).
fn host(
    background: u32,
    opacity: Option<f32>,
    paint: impl FnOnce(&mut Window) + 'static,
) -> AnyElement {
    let canvas = canvas(|_, _, _| (), move |_, _, window, _| paint(window)).size_full();
    let outer = div().size_full().bg(hex(background));
    match opacity {
        Some(opacity) => outer.child(div().size_full().opacity(opacity).child(canvas)),
        None => outer.child(canvas),
    }
    .into_any_element()
}

struct Checkerboard {
    image: Arc<RenderImage>,
}

impl Checkerboard {
    fn new() -> Self {
        Self {
            image: generated_image(82, 42, |x, y| {
                if (x / 4 + y / 4) % 2 == 0 {
                    0x70B8FF
                } else {
                    0xFFC878
                }
            }),
        }
    }
}

impl Scene for Checkerboard {
    fn size(&self) -> (f32, f32) {
        (900.0, 500.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let image = self.image.clone();
        host(0x161B25, Some(0.8), move |window| {
            let area = bounds(40.0, 40.0, 820.0, 420.0);
            let mask = ImageAlphaMask {
                bounds: bounds(180.0, 110.0, 520.0, 230.0),
                radius: px(36.0),
                feather: px(24.0),
                clearance: px(8.0),
                bottom_fade: Some((px(440.0), px(36.0))),
            };
            window
                .paint_image_fitted_masked(
                    area,
                    area,
                    Corners::all(px(24.0)),
                    image,
                    0,
                    false,
                    Some(mask),
                )
                .expect("image paints");
        })
    }
}

struct FittedCrop {
    image: Arc<RenderImage>,
}

impl FittedCrop {
    fn new() -> Self {
        Self {
            image: generated_image(512, 256, |x, _| if x < 256 { 0xE04040 } else { 0x4080E0 }),
        }
    }
}

impl Scene for FittedCrop {
    fn size(&self) -> (f32, f32) {
        (640.0, 400.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let image = self.image.clone();
        host(0x182838, None, move |window| {
            let mask = ImageAlphaMask {
                bounds: bounds(240.0, 150.0, 160.0, 80.0),
                radius: px(20.0),
                feather: px(24.0),
                clearance: px(8.0),
                bottom_fade: None,
            };
            window
                .paint_image_fitted_masked(
                    bounds(120.0, 60.0, 400.0, 280.0),
                    bounds(40.0, 60.0, 560.0, 280.0),
                    Corners::all(px(24.0)),
                    image,
                    0,
                    false,
                    Some(mask),
                )
                .expect("image paints");
        })
    }
}

struct CoverFit {
    image: Arc<RenderImage>,
}

impl CoverFit {
    fn new() -> Self {
        Self {
            image: generated_image(800, 400, |_, y| if y < 200 { 0x8060A0 } else { 0x40A080 }),
        }
    }
}

impl Scene for CoverFit {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let image = self.image.clone();
        host(0x181A1F, None, move |window| {
            let mask = ImageAlphaMask {
                bounds: bounds(240.0, 240.0, 320.0, 200.0),
                radius: px(20.0),
                feather: px(120.0),
                clearance: px(8.0),
                bottom_fade: Some((px(440.0), px(400.0))),
            };
            // Cover fit of an 800 x 400 source into the (80,40) 640 x 400 viewport at zoom 1
            // and focal (0.5, 0.5): the fitted rectangle is (0,40) 800 x 400.
            window
                .paint_image_fitted_masked(
                    bounds(80.0, 40.0, 640.0, 400.0),
                    bounds(0.0, 40.0, 800.0, 400.0),
                    Corners::default(),
                    image,
                    0,
                    false,
                    Some(mask),
                )
                .expect("image paints");
        })
    }
}

register_scene!("FID-RND-004-1", Checkerboard::new);
register_scene!("FID-RND-004-2", FittedCrop::new);
register_scene!("FID-RND-004-3", CoverFit::new);
