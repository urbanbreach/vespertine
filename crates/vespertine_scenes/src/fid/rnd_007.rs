//! FID-RND-007: a 2 x 2 raw BGRA image painted with explicit nearest-neighbor sampling next to
//! an ordinary linear copy.

use std::sync::Arc;

use crate::common::{WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Bounds, Corners, IntoElement, ParentElement, Pixels, RenderImage, Styled,
    Window, canvas, div, point, px, size,
};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

struct NearestBlocks {
    image: Arc<RenderImage>,
}

impl NearestBlocks {
    fn new() -> Self {
        // Red, green / blue, white; BGRA byte order, opaque.
        let bgra: [[u8; 4]; 4] = [
            [0x00, 0x00, 0xFF, 0xFF],
            [0x00, 0xFF, 0x00, 0xFF],
            [0xFF, 0x00, 0x00, 0xFF],
            [0xFF, 0xFF, 0xFF, 0xFF],
        ];
        Self {
            image: Arc::new(
                RenderImage::from_bgra(2, 2, bgra.concat()).expect("valid 2 x 2 image"),
            ),
        }
    }
}

impl Scene for NearestBlocks {
    fn size(&self) -> (f32, f32) {
        (400.0, 200.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let image = self.image.clone();
        div()
            .size_full()
            .bg(hex(WHITE))
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let nearest = bounds(40.0, 40.0, 120.0, 120.0);
                        let linear = bounds(220.0, 40.0, 120.0, 120.0);
                        window
                            .paint_image_nearest(
                                nearest,
                                nearest,
                                Corners::default(),
                                image.clone(),
                                0,
                                false,
                            )
                            .expect("nearest image paints");
                        window
                            .paint_image(linear, linear, Corners::default(), image, 0, false)
                            .expect("linear image paints");
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

register_scene!("FID-RND-007", NearestBlocks::new);
