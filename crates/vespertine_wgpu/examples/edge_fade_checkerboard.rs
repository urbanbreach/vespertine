//! RND-001 A10: one tall glyph and one colored image fade through the `edge_faded` wrapper
//! over a checkerboard, with no flat-color veil.
//!
//! The same tree is rendered three times: the checkerboard alone (C), the faded subtree's
//! content painted without the wrapper (U), and with the wrapper (F). Alpha modulation is
//! linear in the factor, so a faded pixel must equal C + E × (U − C) for every channel, with
//! E the specification's edge coverage; a backdrop-colored veil would break that equality
//! inside the bands. Outside the wrapper's bounds the faded frame must equal the checkerboard
//! exactly (full contrast), and the contrast between adjacent squares is checked explicitly.
//! Exits non-zero on any mismatch.
//!
//! Run with: `cargo run -p vespertine_wgpu --features test-support --example edge_fade_checkerboard`

#[path = "edge_fade_support/mod.rs"]
mod support;

use image::{Frame, Rgba, RgbaImage};
use std::sync::Arc;
use support::{DeviceFade, Host};
use vespertine::{
    AnyElement, IntoElement as _, ParentElement as _, RenderImage, Styled as _, div, edge_faded,
    img, px, rgb, white,
};

const WINDOW: (f32, f32) = (320.0, 200.0);
const SQUARE: f32 = 20.0;
const BOARD: (f32, f32, usize, usize) = (20.0, 20.0, 14, 8);
/// Wrapper bounds in logical pixels: x, y, width, height.
const REGION: (f32, f32, f32, f32) = (40.0, 30.0, 240.0, 140.0);
const BAND: f32 = 30.0;
const DARK: u32 = 0x30343D;
const LIGHT: u32 = 0x586070;
const IMAGE: u32 = 0xD0A060;
const BACKGROUND: u32 = 0x181A1F;

#[derive(Clone, Copy, PartialEq)]
enum Variant {
    BoardOnly,
    Unfaded,
    Faded,
}

fn scene(image: Arc<RenderImage>, variant: Variant) -> AnyElement {
    let board = div()
        .absolute()
        .left(px(BOARD.0))
        .top(px(BOARD.1))
        .flex()
        .flex_col()
        .children((0..BOARD.3).map(|row| {
            div().flex().children((0..BOARD.2).map(move |column| {
                div()
                    .w(px(SQUARE))
                    .h(px(SQUARE))
                    .flex_none()
                    .bg(rgb(if (row + column) % 2 == 0 { DARK } else { LIGHT }))
            }))
        }));
    let content = div()
        .relative()
        .size_full()
        .child(
            img(image)
                .absolute()
                .left(px(10.0))
                .top(px(0.0))
                .w(px(100.0))
                .h(px(REGION.3)),
        )
        .child(
            div()
                .absolute()
                .left(px(130.0))
                .top(px(0.0))
                .w(px(100.0))
                .h(px(REGION.3))
                .text_size(px(150.0))
                .line_height(px(REGION.3))
                .text_color(white())
                .child("H"),
        );
    let region = |child: AnyElement| {
        div()
            .absolute()
            .left(px(REGION.0))
            .top(px(REGION.1))
            .w(px(REGION.2))
            .h(px(REGION.3))
            .child(child)
    };
    let root = div().size_full().bg(rgb(BACKGROUND)).child(board);
    match variant {
        Variant::BoardOnly => root,
        Variant::Unfaded => root.child(region(content.into_any_element())),
        Variant::Faded => root.child(region(
            edge_faded(px(BAND), true, true, content)
                .fade_left(true)
                .fade_right(true)
                .into_any_element(),
        )),
    }
    .into_any_element()
}

fn channels(color: u32) -> [f32; 3] {
    [16, 8, 0].map(|shift| ((color >> shift) & 0xFF) as f32)
}

fn main() {
    let (red, green, blue) = (
        ((IMAGE >> 16) & 0xFF) as u8,
        ((IMAGE >> 8) & 0xFF) as u8,
        (IMAGE & 0xFF) as u8,
    );
    // Image frames are BGRA.
    let image = Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::from_pixel(
        100,
        REGION.3 as u32,
        Rgba([blue, green, red, 255]),
    ))]));
    let mut host = Host::new();
    let mut failures = 0;
    for scale in [1.0f32, 2.0] {
        let render = |host: &mut Host, variant| {
            let image = image.clone();
            host.render(WINDOW.0, WINDOW.1, scale, move |_, _| {
                scene(image.clone(), variant)
            })
        };
        let board = render(&mut host, Variant::BoardOnly);
        let unfaded = render(&mut host, Variant::Unfaded);
        let faded = render(&mut host, Variant::Faded);
        let fade = DeviceFade {
            top: REGION.1 * scale,
            bottom: (REGION.1 + REGION.3) * scale,
            left: REGION.0 * scale,
            right: (REGION.0 + REGION.2) * scale,
            bands: [BAND * scale; 4],
        };
        let (width, height) = faded.dimensions();
        let mut worst_inside = 0.0f32;
        let mut worst_outside = 0.0f32;
        let mut band_pixels = 0usize;
        let mut veil_distance = 0.0f32;
        for y in 0..height {
            for x in 0..width {
                let inside = (x as f32 + 0.5) > fade.left
                    && (x as f32 + 0.5) < fade.right
                    && (y as f32 + 0.5) > fade.top
                    && (y as f32 + 0.5) < fade.bottom;
                let c = board.get_pixel(x, y).0;
                let u = unfaded.get_pixel(x, y).0;
                let f = faded.get_pixel(x, y).0;
                let coverage = if inside { fade.coverage(x, y) } else { 0.0 };
                if inside && coverage > 0.05 && coverage < 0.95 && u != c {
                    band_pixels += 1;
                }
                for channel in 0..3 {
                    let want = f32::from(c[channel])
                        + coverage * (f32::from(u[channel]) - f32::from(c[channel]));
                    let error = (f32::from(f[channel]) - want).abs();
                    if inside {
                        worst_inside = worst_inside.max(error);
                    } else {
                        worst_outside = worst_outside.max(error);
                    }
                    // Distance from what a flat-color veil (the window background shown through
                    // the same E) would produce: this must not be small where content fades.
                    if inside && coverage > 0.05 && coverage < 0.5 && u != c {
                        let veil =
                            0x18 as f32 * (1.0 - coverage) + f32::from(u[channel]) * coverage;
                        veil_distance = veil_distance.max((f32::from(f[channel]) - veil).abs());
                    }
                }
            }
        }
        // Full contrast outside the band: adjacent squares at the board's top-left corner
        // (outside the wrapper) differ by exactly the checkerboard's own contrast.
        let sample = |image: &RgbaImage, column: usize, row: usize| {
            let x = ((BOARD.0 + SQUARE * (column as f32 + 0.5)) * scale) as u32;
            let y = ((BOARD.1 + SQUARE * (row as f32 + 0.5)) * scale) as u32;
            image.get_pixel(x, y).0
        };
        let (a, b) = (sample(&faded, 0, 0), sample(&faded, 1, 0));
        let contrast = (0..3)
            .map(|channel| (channels(LIGHT)[channel] - channels(DARK)[channel]).abs())
            .fold(0.0f32, f32::max);
        let measured = (0..3)
            .map(|channel| (f32::from(b[channel]) - f32::from(a[channel])).abs())
            .fold(0.0f32, f32::max);
        let ok = worst_inside <= 3.0
            && worst_outside <= 1.0
            && band_pixels > 100
            && veil_distance > 8.0
            && (measured - contrast).abs() <= 2.0;
        println!(
            "scale {scale}: inside max error {worst_inside:.2}/255, outside {worst_outside:.2}/255, \
             {band_pixels} faded content pixels, distance from a veil {veil_distance:.1}/255, \
             outside-contrast {measured:.0} (checkerboard {contrast:.0}): {}",
            if ok { "ok" } else { "FAILED" }
        );
        if !ok {
            failures += 1;
        }
    }
    if failures > 0 {
        std::process::exit(1);
    }
    println!("fade modulates alpha only: no veil");
}
