//! RND-004 A7: a non-square generated image is cropped into a square visible rectangle with
//! intact rounding on all four visible corners; a second copy shows the feathered inverse
//! rounded-rectangle cutout over the same cover-fit geometry.
//!
//! Renders headlessly through the GPU renderer, asserts the pixels and optionally writes the
//! frame as a PNG:
//!
//! ```sh
//! cargo run -p vespertine --example image_crop_mask [output.png]
//! ```

#[path = "example_support/image_scene.rs"]
mod image_scene;

use image_scene::*;
use vespertine::{Corners, ImageAlphaMask, cover_fit, point, px, size};

const BACKGROUND: u32 = 0x182838;
const RED: u32 = 0xE04040;
const BLUE: u32 = 0x4080E0;

fn main() {
    // A 512 x 256 source (red left half, blue right half) into two 240 x 240 squares.
    let image = fid_004_2_image();
    let plain = logical_bounds(40.0, 80.0, 240.0, 240.0);
    let masked = logical_bounds(360.0, 80.0, 240.0, 240.0);
    let mask = ImageAlphaMask {
        bounds: logical_bounds(420.0, 150.0, 120.0, 100.0),
        radius: px(16.0),
        feather: px(20.0),
        clearance: px(6.0),
        bottom_fade: None,
    };
    let mut scene = PaintView::scene(size(px(640.0), px(400.0)), BACKGROUND, 1.0, move |window| {
        for (visible, mask) in [(plain, None), (masked, Some(mask))] {
            // Cover fit: the 512 x 256 source scales by 240 / 256 and overflows horizontally.
            let fitted = cover_fit(visible, size(512.0, 256.0), 1.0, point(0.5, 0.5))
                .expect("valid cover fit");
            window
                .paint_image_fitted_masked(
                    visible,
                    fitted,
                    Corners::all(px(24.0)),
                    image.clone(),
                    0,
                    false,
                    mask,
                )
                .expect("image paints");
        }
    });
    let capture = scene.capture();

    for (left, top) in [(40.0, 80.0), (360.0, 80.0)] {
        let (right, bottom) = (left + 240.0, top + 240.0);
        // The crop shows the middle 256 x 256 texels: red left of the center, blue right.
        assert!(near(capture.at(left + 30.0, top + 120.0), RED, 3));
        assert!(near(capture.at(left + 210.0, top + 120.0), BLUE, 3));
        // All four visible corners are rounded (radius 24): the corner pixel is background...
        for (x, y) in [
            (left + 0.5, top + 0.5),
            (right - 0.5, top + 0.5),
            (left + 0.5, bottom - 0.5),
            (right - 0.5, bottom - 0.5),
        ] {
            assert!(
                near(capture.at(x, y), BACKGROUND, 2),
                "corner ({x}, {y}) is not rounded"
            );
        }
        // ...while the pixel 8 logical pixels in on both axes (inside the arc) is image.
        for (x, y) in [
            (left + 8.0, top + 8.0),
            (right - 8.0, top + 8.0),
            (left + 8.0, bottom - 8.0),
            (right - 8.0, bottom - 8.0),
        ] {
            let color = capture.at(x, y);
            assert!(
                near(color, RED, 3) || near(color, BLUE, 3),
                "({x}, {y}) lost its image: {color:?}"
            );
        }
    }
    // The cutout of the second copy removes its center and leaves the far corner.
    assert!(near(capture.at(480.0, 200.0), BACKGROUND, 3));
    assert!(near(capture.at(580.0, 300.0), BLUE, 3));

    if let Some(path) = std::env::args().nth(1) {
        capture.image.save(&path).expect("writes the PNG");
        println!("wrote {path}");
    }
    println!("image_crop_mask: square crop keeps all four rounded corners; cutout verified");
}
