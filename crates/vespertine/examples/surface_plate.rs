//! RND-008: the same layered surface plate painted twice, once as a styled element and once
//! from a canvas in the canonical layer order (drop shadows, fill, inset shadows, content,
//! border). Renders FID-RND-008 headlessly, compares the two plates with the standard
//! tolerance (a channel differs above 8/255, at most 0.5% of pixels may differ) and exits
//! non-zero on mismatch. `--out <dir>` also writes the screenshot.

#[path = "example_support/headless_host.rs"]
mod headless_host;
#[path = "example_support/surface_plate.rs"]
mod surface_plate;

use vespertine::{IntoElement, ParentElement};

fn main() {
    let mut host = headless_host::Host::new();
    let image = host.render(800., 480., |_, _| {
        surface_plate::scene_root()
            .child(surface_plate::styled_plate(80., 80.))
            .child(surface_plate::canvas_plate(420., 80.))
            .into_any_element()
    });
    headless_host::save(&headless_host::output_directory(), "surface_plate", &image);

    // Logical x in [40, 360) of the styled plate against [380, 700) of the canvas plate,
    // shadow margins included; device pixels are twice the logical values.
    let (width, height) = (640u32, 400u32);
    let mut differing = 0u32;
    for y in 0..height {
        for x in 0..width {
            let styled = image.get_pixel(80 + x, 100 + y).0;
            let canvas = image.get_pixel(760 + x, 100 + y).0;
            if styled.iter().zip(canvas).any(|(a, b)| a.abs_diff(b) > 8) {
                differing += 1;
            }
        }
    }
    let fraction = differing as f32 / (width * height) as f32;
    println!(
        "styled plate vs canvas plate: {differing} of {} pixels differ ({:.4}%)",
        width * height,
        fraction * 100.0
    );
    if fraction > 0.005 {
        eprintln!("FAIL: more than 0.5% of pixels differ");
        std::process::exit(1);
    }
    println!("OK");
}
