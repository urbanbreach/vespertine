//! Renderer memory accounting (DEV-002): run with `VESPERTINE_RENDERER_MEMORY=1` and watch the
//! one-line reports on standard error (the first completed frame, then at most one per ten
//! seconds). The example grows the glyph/image atlas with many images, engages a backdrop blur
//! so scratch storage appears, then paints effect-free frames so the storage is released after
//! thirty frames. No application logger is installed.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::{sync::Arc, time::Instant};

use image::{Frame, ImageBuffer, Rgba};
use vespertine::{
    App, AppContext, Bounds, Context, Corners, ImageSource, IntoElement, ParentElement,
    RenderImage, Styled, Window, WindowBounds, WindowOptions, canvas, div, fill, hsla, img, point,
    px, rgb, size,
};
use vespertine_platform::application;

/// Seconds of the atlas-growth phase, then of the blur phase, then of the effect-free phase.
const GROWTH_SECONDS: f32 = 12.0;
const BLUR_SECONDS: f32 = 30.0;
const TOTAL_SECONDS: f32 = 45.0;

struct MemoryDemo {
    start: Instant,
    images: Vec<Arc<RenderImage>>,
}

impl MemoryDemo {
    fn new() -> Self {
        let images = (0..320u32)
            .map(|index| {
                let buffer = ImageBuffer::from_fn(128, 128, |x, y| {
                    Rgba([(x * 2) as u8, (y * 2) as u8, (index * 3) as u8, 255])
                });
                Arc::new(RenderImage::new(vec![Frame::new(buffer)]))
            })
            .collect();
        Self {
            start: Instant::now(),
            images,
        }
    }
}

impl vespertine::Render for MemoryDemo {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let elapsed = self.start.elapsed().as_secs_f32();
        if elapsed < TOTAL_SECONDS {
            window.request_animation_frame();
        }

        let mut root = div().relative().size_full().bg(rgb(0x101820));
        if elapsed < GROWTH_SECONDS {
            for (index, image) in self.images.iter().enumerate() {
                let (column, row) = (index % 20, index / 20);
                root = root.child(
                    img(ImageSource::Render(image.clone()))
                        .absolute()
                        .left(px(10. + 36. * column as f32))
                        .top(px(10. + 36. * row as f32))
                        .w(px(32.))
                        .h(px(32.)),
                );
            }
        } else if elapsed < BLUR_SECONDS {
            let stripes = canvas(
                |_, _, _| (),
                |bounds, _, window, _| {
                    let mut x = bounds.origin.x;
                    while x < bounds.right() {
                        window.paint_quad(fill(
                            Bounds {
                                origin: point(x, bounds.origin.y),
                                size: size(px(20.), bounds.size.height),
                            },
                            rgb(0xF0C040),
                        ));
                        x += px(40.);
                    }
                },
            )
            .absolute()
            .size_full();
            let blur = canvas(
                |_, _, _| (),
                |bounds, _, window, _| {
                    window.paint_backdrop_blur(bounds, Corners::all(px(16.)), px(16.));
                },
            )
            .absolute()
            .size_full();
            root = root.child(stripes).child(
                div()
                    .absolute()
                    .left(px(80.))
                    .top(px(60.))
                    .w(px(320.))
                    .h(px(200.))
                    .child(blur)
                    .child(
                        div()
                            .absolute()
                            .size_full()
                            .rounded(px(16.))
                            .bg(hsla(0., 0., 1., 0.1)),
                    ),
            );
        }
        root
    }
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(760.), px(520.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| MemoryDemo::new()),
        )
        .unwrap();
        cx.activate(true);
    });
}
