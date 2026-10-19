//! RND-009: canvas painting. Renders FID-RND-009-1 (open round-capped polyline, triangle,
//! circle and text from one canvas) and FID-RND-009-2 (canvas text against ordinary text
//! under the same clip), checks the primitives landed where described and that the two text
//! panels are equal within the standard tolerance, and exits non-zero on mismatch.
//! `--out <dir>` also writes the screenshots.

#[path = "example_support/canvas_scenes.rs"]
mod canvas_scenes;
#[path = "example_support/headless_host.rs"]
mod headless_host;

fn main() {
    let mut host = headless_host::Host::new();
    let out = headless_host::output_directory();

    let vector = host.render(800., 480., |_, _| canvas_scenes::vector_scene());
    headless_host::save(&out, "canvas_vector", &vector);
    let background = vector.get_pixel(2, 2).0;
    let differs = |x: f32, y: f32| {
        let pixel = vector.get_pixel((x * 2.0) as u32, (y * 2.0) as u32).0;
        pixel
            .iter()
            .zip(background)
            .any(|(a, b)| a.abs_diff(b) > 20)
    };
    let mut ok = true;
    let mut check = |name: &str, condition: bool| {
        println!("{name}: {}", if condition { "ok" } else { "FAILED" });
        ok &= condition;
    };
    check("polyline body", differs(120., 90.));
    check("polyline start cap", differs(79.5, 120.5));
    check(
        "polyline is open (no closing segment)",
        !differs(160., 120.),
    );
    check("triangle", differs(400., 100.));
    check("circle", differs(600., 90.));
    check("circle bounds", !differs(600., 50.));

    let panels = host.render(800., 480., |_, _| canvas_scenes::text_clip_scene());
    headless_host::save(&out, "canvas_text_panels", &panels);
    let (width, height) = (600u32, 120u32);
    let mut differing = 0u32;
    for y in 0..height {
        for x in 0..width {
            let ordinary = panels.get_pixel(160 + x, 600 + y).0;
            let canvas = panels.get_pixel(840 + x, 600 + y).0;
            if ordinary.iter().zip(canvas).any(|(a, b)| a.abs_diff(b) > 8) {
                differing += 1;
            }
        }
    }
    println!("canvas text vs ordinary text: {differing} differing pixels");
    check(
        "canvas text equals ordinary text",
        differing * 200 <= width * height,
    );
    // The clip cuts the text at the panel edge (x = 380 logical): the pixel just outside is
    // the window background.
    let cut = panels.get_pixel(2 * 381, 2 * 330).0;
    check(
        "text cut at the clip edge",
        cut == panels.get_pixel(2 * 395, 2 * 330).0,
    );
    if !ok {
        std::process::exit(1);
    }
    println!("OK");
}
