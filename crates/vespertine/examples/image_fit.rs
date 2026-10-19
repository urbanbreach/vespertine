//! ELM-003: image fitting with element-space rounded corners.
//!
//! The generated source is 320 x 160 pixels with eight vertical 40 px stripes, alternating
//! white and #0080FF. Top row (the FID-ELM-003 scene): 160 x 160 elements with an explicit
//! size and radius 24, cover on the left (the visible crop spans source columns 80 to 240,
//! four stripes) and contain on the right (a 160 x 80 rectangle with its own rounded corners).
//! Bottom row: a 100 x 100 cover element with radius 16 showing the middle half of the source
//! columns (25% to 75%) with four fully rounded visible corners, then the same with contain.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::sync::Arc;

use vespertine::{
    App, Bounds, Context, ImageSource, ObjectFit, PixelFormat, RenderImage, Window, WindowBounds,
    WindowOptions, div, img, prelude::*, px, render_image_from_pixels, rgb, size,
};
use vespertine_platform::application;

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
            .expect("valid generated pixels"),
    )
}

struct ImageFit {
    image: Arc<RenderImage>,
}

impl ImageFit {
    fn thumbnail(
        &self,
        side: f32,
        radius: f32,
        fit: ObjectFit,
        x: f32,
        y: f32,
    ) -> impl IntoElement {
        div().absolute().left(px(x)).top(px(y)).child(
            img(ImageSource::Render(self.image.clone()))
                .w(px(side))
                .h(px(side))
                .rounded(px(radius))
                .object_fit(fit),
        )
    }
}

impl Render for ImageFit {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(rgb(0x202020))
            .child(self.thumbnail(160., 24., ObjectFit::Cover, 20., 20.))
            .child(self.thumbnail(160., 24., ObjectFit::Contain, 220., 20.))
            .child(self.thumbnail(100., 16., ObjectFit::Cover, 20., 200.))
            .child(self.thumbnail(100., 16., ObjectFit::Contain, 140., 200.))
    }
}

fn run_example() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(400.), px(320.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| ImageFit { image: stripes() }),
        )
        .unwrap();
        cx.activate(true);
    });
}

fn main() {
    run_example();
}
