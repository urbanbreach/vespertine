//! FID-ELM-003: image fitting with element-space rounded corners.
//!
//! A generated opaque 320 x 160 source of eight 40 px vertical stripes (white and #0080FF) in two
//! 160 x 160 elements with an explicit size and radius 24: cover on the left (visible crop spans
//! source columns 80 to 240) and contain on the right (a 160 x 80 rectangle with its own rounded
//! corners).

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{
    AnyElement, App, ImageSource, IntoElement, ObjectFit, ParentElement, PixelFormat, RenderImage,
    Styled, StyledImage, Window, div, img, px, render_image_from_pixels,
};

struct Elm003 {
    image: Arc<RenderImage>,
}

fn stripes() -> Arc<RenderImage> {
    let (width, height) = (320usize, 160usize);
    let mut pixels = Vec::with_capacity(width * height * 4);
    for _ in 0..height {
        for x in 0..width {
            let color: [u8; 4] = if (x / 40) % 2 == 0 {
                [0xFF, 0xFF, 0xFF, 0xFF]
            } else {
                [0x00, 0x80, 0xFF, 0xFF]
            };
            pixels.extend_from_slice(&color);
        }
    }
    Arc::new(
        render_image_from_pixels(width as f64, height as f64, &pixels, PixelFormat::Rgba)
            .expect("generated pixels are valid"),
    )
}

impl Elm003 {
    fn thumbnail(&self, fit: ObjectFit, x: f32) -> impl IntoElement {
        div().absolute().left(px(x)).top(px(20.)).child(
            img(ImageSource::Render(self.image.clone()))
                .w(px(160.))
                .h(px(160.))
                .rounded(px(24.))
                .object_fit(fit),
        )
    }
}

impl Scene for Elm003 {
    fn size(&self) -> (f32, f32) {
        (400.0, 220.0)
    }

    fn frame(&mut self, _frame: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .relative()
            .size_full()
            .bg(hex(0x202020))
            .child(self.thumbnail(ObjectFit::Cover, 20.))
            .child(self.thumbnail(ObjectFit::Contain, 220.))
            .into_any_element()
    }
}

register_scene!("FID-ELM-003", || Elm003 { image: stripes() });
