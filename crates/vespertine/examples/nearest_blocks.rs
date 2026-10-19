//! RND-007 A2 (FID-RND-007): a 2 x 2 raw BGRA image painted with explicit nearest-neighbor
//! sampling next to an ordinary linear copy. The nearest copy shows four flat 60 x 60 logical
//! pixel blocks with hard edges; the linear copy blends smoothly.
//!
//! Renders headlessly through the GPU renderer, asserts the pixels and optionally writes the
//! frame as a PNG:
//!
//! ```sh
//! cargo run -p vespertine --example nearest_blocks [output.png]
//! ```

#[path = "example_support/image_scene.rs"]
mod image_scene;

use image_scene::*;
use vespertine::{px, size};

fn main() {
    let image = fid_007_image();
    let mut scene = PaintView::scene(size(px(400.0), px(200.0)), 0xFFFFFF, 1.0, move |window| {
        paint_fid_007(window, &image)
    });
    let capture = scene.capture();
    check_fid_007(&capture);
    if let Some(path) = std::env::args().nth(1) {
        capture.image.save(&path).expect("writes the PNG");
        println!("wrote {path}");
    }
    println!("nearest_blocks: flat hard-edged nearest blocks and a smooth linear copy verified");
}
