//! FID-ELM-004-1 (object fit and tinted vector hosts) and FID-ELM-004-2 (color-preserving versus
//! monochrome vector images).

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{
    AnyElement, App, Image, ImageFormat, ImageHostState, ImageSource, IntoElement, ParentElement,
    PixelFormat, RenderImage, StyleRefinement, Styled, SvgHostState, Window, div, img, px,
    render_image_from_pixels, rgb, svg,
};

const DISC: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100" viewBox="0 0 100 100"><circle cx="50" cy="50" r="30"/></svg>"#;

struct Elm004Hosts {
    raster: ImageHostState,
    vector: SvgHostState,
}

impl Elm004Hosts {
    fn new() -> Self {
        // 100 x 50 pixels: the left 50 columns opaque #FF0000, the right 50 opaque #0000FF.
        let mut pixels = Vec::with_capacity(100 * 50 * 4);
        for _ in 0..50 {
            for x in 0..100 {
                let color: [u8; 4] = if x < 50 {
                    [0xFF, 0x00, 0x00, 0xFF]
                } else {
                    [0x00, 0x00, 0xFF, 0xFF]
                };
                pixels.extend_from_slice(&color);
            }
        }
        let mut raster = ImageHostState::new();
        raster
            .set_image_pixels(100., 50., &pixels, Some("rgba"))
            .expect("generated pixels are valid");
        let mut vector = SvgHostState::new();
        vector.set_source(Some(DISC));
        Self { raster, vector }
    }
}

impl Scene for Elm004Hosts {
    fn size(&self) -> (f32, f32) {
        (480.0, 260.0)
    }

    fn frame(&mut self, _frame: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        let raster_style = StyleRefinement::default()
            .w(px(200.))
            .h(px(200.))
            .bg(rgb(0x303030));
        let vector_style = StyleRefinement::default()
            .w(px(100.))
            .h(px(100.))
            .text_color(rgb(0x60A5FA));
        let raster = self.raster.element("raster", raster_style, window, cx);
        let vector = self.vector.element("vector", vector_style, cx);
        div()
            .relative()
            .size_full()
            .bg(hex(0x181818))
            .child(div().absolute().left(px(20.)).top(px(20.)).child(raster))
            .child(div().absolute().left(px(260.)).top(px(20.)).child(vector))
            .into_any_element()
    }
}

register_scene!("FID-ELM-004-1", Elm004Hosts::new);

const TWO_COLOR: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="80" viewBox="0 0 80 80"><rect x="0" width="40" height="80" fill="#7DCFFF"/><rect x="40" width="40" height="80" fill="#D0A060"/></svg>"##;

const HOLE: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="80" viewBox="0 0 80 80"><path fill-rule="evenodd" d="M0 0H80V80H0Z M20 20V60H60V20Z"/></svg>"##;

struct Elm004Icons {
    color_icon: Arc<Image>,
    raster: Arc<RenderImage>,
}

impl Elm004Icons {
    fn new() -> Self {
        // 160 x 80 pixels: the left 80 columns opaque #D0A060, the right 80 opaque #7DCFFF.
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
        Self {
            color_icon: Arc::new(Image::from_bytes(
                ImageFormat::Svg,
                TWO_COLOR.as_bytes().to_vec(),
            )),
            raster: Arc::new(
                render_image_from_pixels(160., 80., &pixels, PixelFormat::Rgba)
                    .expect("generated pixels are valid"),
            ),
        }
    }
}

impl Scene for Elm004Icons {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _frame: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .relative()
            .size_full()
            .bg(hex(0x181A1F))
            .child(
                div().absolute().left(px(80.)).top(px(80.)).child(
                    img(ImageSource::Image(self.color_icon.clone()))
                        .w(px(80.))
                        .h(px(80.)),
                ),
            )
            .child(
                svg()
                    .data(HOLE.as_bytes())
                    .absolute()
                    .left(px(240.))
                    .top(px(80.))
                    .w(px(80.))
                    .h(px(80.))
                    .text_color(rgb(0x80C090)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(400.))
                    .top(px(80.))
                    .child(img(ImageSource::Render(self.raster.clone()))),
            )
            .into_any_element()
    }
}

register_scene!("FID-ELM-004-2", Elm004Icons::new);
