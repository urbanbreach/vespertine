//! ELM-004 (FID-ELM-004-2): color-preserving versus monochrome vector images.
//!
//! Left: a two-color SVG through the color-preserving image path (both authored colors
//! visible). Middle: a glyph with an even-odd hole through the monochrome path, tinted with the
//! text color (the hole shows the background). Right: a 160 x 80 raster at its natural size.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::sync::Arc;

use vespertine::{
    App, Bounds, Context, Image, ImageFormat, ImageSource, PixelFormat, RenderImage, Window,
    WindowBounds, WindowOptions, div, img, prelude::*, px, render_image_from_pixels, rgb, size,
    svg,
};
use vespertine_platform::application;

const TWO_COLOR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="80" viewBox="0 0 80 80"><rect x="0" width="40" height="80" fill="#7DCFFF"/><rect x="40" width="40" height="80" fill="#D0A060"/></svg>"##;

const GLYPH: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="80" viewBox="0 0 80 80"><path fill-rule="evenodd" d="M0 0H80V80H0Z M20 20V60H60V20Z"/></svg>"##;

fn raster() -> Arc<RenderImage> {
    let mut pixels = Vec::with_capacity(160 * 80 * 4);
    for _ in 0..80 {
        for x in 0..160 {
            let color: [u8; 4] = if x < 80 {
                [0xD0, 0xA0, 0x60, 0xFF]
            } else {
                [0x7D, 0xCF, 0xFF, 0xFF]
            };
            pixels.extend_from_slice(&color);
        }
    }
    Arc::new(render_image_from_pixels(160., 80., &pixels, PixelFormat::Rgba).expect("valid pixels"))
}

struct MediaIcons {
    color_preserving: Arc<Image>,
    raster: Arc<RenderImage>,
}

impl Render for MediaIcons {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .bg(rgb(0x181A1F))
            .child(
                div().absolute().left(px(80.)).top(px(80.)).child(
                    img(ImageSource::Image(self.color_preserving.clone()))
                        .w(px(80.))
                        .h(px(80.)),
                ),
            )
            .child(
                svg()
                    .data(GLYPH.as_bytes())
                    .absolute()
                    .left(px(240.))
                    .top(px(80.))
                    .size(px(80.))
                    .text_color(rgb(0x80C090)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(400.))
                    .top(px(80.))
                    .child(img(ImageSource::Render(self.raster.clone()))),
            )
    }
}

fn run_example() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(800.), px(480.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| MediaIcons {
                    color_preserving: Arc::new(Image::from_bytes(
                        ImageFormat::Svg,
                        TWO_COLOR.as_bytes().to_vec(),
                    )),
                    raster: raster(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

fn main() {
    run_example();
}
