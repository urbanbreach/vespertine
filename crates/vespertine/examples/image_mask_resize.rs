//! RND-004 A13: an input-like surface laid over an image excludes a feathered rounded
//! rectangle from it. The mask is taken from the surface bounds measured in the same prepaint
//! pass as the image region and extends to the image bottom, so resizing the surface moves the
//! mask on the same frame (no one-frame lag) without decoding or creating a new image identity.
//!
//! Renders headlessly through the GPU renderer and asserts every frame:
//!
//! ```sh
//! cargo run -p vespertine --example image_mask_resize [output.png]
//! ```

#[path = "example_support/image_scene.rs"]
mod image_scene;

use std::{cell::Cell, rc::Rc};

use image_scene::*;
use vespertine::{AppContext as _, px, size};

fn main() {
    let image = resize_scene_image();
    let height = Rc::new(Cell::new(40.0f32));
    let (scene_image, scene_height) = (image, height.clone());
    let mut scene = HeadlessScene::new(
        size(px(RESIZE_WINDOW.0), px(RESIZE_WINDOW.1)),
        move |_, cx| cx.new(|_| ResizeScene::new(scene_image, scene_height)),
    );
    let image_bottom = (RESIZE_IMAGE_AREA.1 + RESIZE_IMAGE_AREA.3) * 2.0;

    for surface_height in [40.0, 72.0, 104.0, 56.0, 88.0] {
        height.set(surface_height);
        scene.draw();
        let surface = scene.with_window(|window, _| painted_surface_quad(window));
        let sprite = scene.with_window(|window, _| window.painted_polychrome_sprites())[0];
        let mask = sprite.effects.mask_bounds;
        // Same frame: the mask is exactly the surface's left edge, top edge and width, and
        // reaches the image bottom.
        assert_eq!(surface[3], surface_height * 2.0);
        assert_eq!(
            [mask[0], mask[1], mask[2]],
            [surface[0], surface[1], surface[2]]
        );
        assert_eq!(mask[1] + mask[3], image_bottom);
        // The image was decoded and uploaded once, on the first frame, and never again.
        assert_eq!(scene.counters.insertions(), 1);
        println!(
            "surface height {surface_height:>5}: mask top {:>5} bottom {:>5} (device px), atlas insertions {}",
            mask[1],
            mask[1] + mask[3],
            scene.counters.insertions()
        );
    }
    assert_eq!(scene.counters.uploaded_bytes(), 800 * 400 * 4);

    let capture = scene.capture();
    assert!(
        near(capture.at(300.0, 330.0), 0x181A1F, 4),
        "artwork must not reappear below the surface"
    );
    assert!(near(capture.at(300.0, 300.0), 0x2A2E38, 3));
    if let Some(path) = std::env::args().nth(1) {
        capture.image.save(&path).expect("writes the PNG");
        println!("wrote {path}");
    }
    println!("image_mask_resize: the mask followed every resize on the same frame");
}
