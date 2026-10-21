//! The plain-solid quirk of the rounded clip scope (RND-002 R6, A2), decided per draw batch
//! (PRF-008 R3). FID-RND-002-1: a quad whose fill is a single solid color, with square
//! corners, no border and no fade edge, alone in its batch bypasses the rounded coverage and
//! keeps square corners while text and images in the scope are rounded. FID-RND-002-5: the
//! same fill in a batch that also holds a gradient quad is drawn by the general route and is
//! rounded. To round a solid fill, force the general route (a gradient, a nonzero radius or
//! border, or a fade edge); never rely on the scope to leave it square.
//!
//! ```sh
//! cargo run -p vespertine --features test-support --example rounded_clip_plain_solid -- --out /tmp/solid
//! ```

#[path = "example_support/headless_host.rs"]
mod headless_host;

use headless_host::{Host, output_directory, save, scope};
use image::{Frame, ImageBuffer, Rgba};
use std::sync::Arc;
use vespertine::{
    AnyElement, Background, Bounds, Corners, IntoElement, ParentElement, RenderImage, Styled,
    canvas, div, fill, linear_color_stop, linear_gradient, point, px, rgb, rounded_clip, size,
    white,
};

const RADIUS: f32 = 36.0;
/// Left (solid fill) and right (gradient fill) scopes: left, top, width, height.
const LEFT: [f32; 4] = [20.0, 30.0, 170.0, 140.0];
const RIGHT: [f32; 4] = [210.0, 30.0, 170.0, 140.0];
const FILL: u32 = 0xe0a040;
const BACKGROUND: u32 = 0x181a1f;

fn solid() -> Background {
    rgb(FILL).into()
}

fn same_color_gradient() -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgb(FILL), 0.0),
        linear_color_stop(rgb(FILL), 1.0),
    )
}

fn image() -> Arc<RenderImage> {
    let buffer = ImageBuffer::from_fn(40, 40, |x, y| {
        Rgba([220, (x * 6) as u8, (y * 6) as u8, 255])
    });
    Arc::new(RenderImage::new(vec![Frame::new(buffer)]))
}

/// One scope over `area` with a window-sized fill, and optionally text and an image that
/// both cross the scope's corners.
fn panel(area: [f32; 4], background: Background, decorations: bool) -> AnyElement {
    let [left, top, width, height] = area;
    let picture = image();
    let mut content = div().size_full().child(
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                // Larger than the scope: the rectangular mask cuts it to the scope bounds.
                let everything = Bounds {
                    origin: point(px(0.0), px(0.0)),
                    size: size(px(400.0), px(200.0)),
                };
                window.paint_quad(fill(everything, background));
            },
        )
        .absolute()
        .size_full(),
    );
    if decorations {
        content = content
            .child(
                div()
                    .absolute()
                    .left(px(2.0))
                    .top(px(-4.0))
                    .text_color(white())
                    .text_size(px(30.0))
                    .child("Rounded"),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        let bounds = Bounds {
                            origin: point(px(left + width - 26.0), px(top + height - 26.0)),
                            size: size(px(40.0), px(40.0)),
                        };
                        window
                            .paint_image(bounds, bounds, Corners::default(), picture, 0, false)
                            .expect("paint image");
                    },
                )
                .absolute()
                .size_full(),
            );
    }
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(width))
        .h(px(height))
        .child(rounded_clip(px(RADIUS), content))
        .into_any_element()
}

/// A window with the solid-fill scope and/or the gradient-fill scope. Their fills are cut by
/// the scopes' rectangular masks to non-overlapping rectangles, so with both present they
/// share one quad batch.
fn window(solid_panel: bool, gradient_panel: bool, decorations: bool) -> AnyElement {
    let mut window = div().size_full().bg(rgb(BACKGROUND));
    if solid_panel {
        window = window.child(panel(LEFT, solid(), decorations));
    }
    if gradient_panel {
        window = window.child(panel(RIGHT, same_color_gradient(), decorations));
    }
    window.into_any_element()
}

fn device_scope(area: [f32; 4]) -> vespertine::ScopeParams {
    let [left, top, width, height] = area;
    scope(
        [
            left * 2.0,
            top * 2.0,
            (left + width) * 2.0,
            (top + height) * 2.0,
        ],
        RADIUS * 2.0,
    )
}

fn main() {
    let output = output_directory();
    let mut host = Host::new();
    let rgb_of = |value: u32| [(value >> 16) as u8, (value >> 8) as u8, value as u8];
    let near = |pixel: &Rgba<u8>, expected: [u8; 3]| {
        pixel.0[..3]
            .iter()
            .zip(expected)
            .all(|(a, b)| a.abs_diff(b) <= 2)
    };
    // A pixel in the cut-away top-left corner of each scope (device pixels): three pixels in
    // from each scope's corner, far outside the radius-72 curve.
    let corner = |image: &image::RgbaImage, area: [f32; 4]| {
        *image.get_pixel(area[0] as u32 * 2 + 3, area[1] as u32 * 2 + 3)
    };

    // FID-RND-002-1: the solid fill alone in its batch keeps its square corner.
    let alone_fills = host.render(400.0, 200.0, |_, _| window(true, false, false));
    let alone_full = host.render(400.0, 200.0, |_, _| window(true, false, true));
    save(&output, "solid_alone_fills_only", &alone_fills);
    save(&output, "solid_alone_with_text_and_image", &alone_full);
    let square = corner(&alone_fills, LEFT);
    assert!(
        near(&square, rgb_of(FILL)),
        "a solid fill alone in its batch must stay square: {square:?}"
    );

    // FID-RND-002-5: with a gradient quad in the batch both fills are rounded.
    let mixed_fills = host.render(400.0, 200.0, |_, _| window(true, true, false));
    let mixed_full = host.render(400.0, 200.0, |_, _| window(true, true, true));
    save(&output, "mixed_batch_fills_only", &mixed_fills);
    save(&output, "mixed_batch_with_text_and_image", &mixed_full);
    let solid_corner = corner(&mixed_fills, LEFT);
    let gradient_corner = corner(&mixed_fills, RIGHT);
    assert!(
        near(&solid_corner, rgb_of(BACKGROUND)),
        "a solid fill sharing a batch with a gradient quad must be rounded: {solid_corner:?}"
    );
    assert!(
        near(&gradient_corner, rgb_of(BACKGROUND)),
        "the gradient fill must be rounded: {gradient_corner:?}"
    );
    println!(
        "alone: solid fill corner {square:?} (square); mixed batch: solid fill corner \
         {solid_corner:?} and gradient fill corner {gradient_corner:?} (both rounded)"
    );

    // Text and images are rounded in every scope, whatever the fill underneath: outside the
    // curve the decorated frame equals the fills-only frame, inside the decorations painted.
    for (name, area, reference, full) in [
        ("solid alone", LEFT, &alone_fills, &alone_full),
        ("solid in a mixed batch", LEFT, &mixed_fills, &mixed_full),
        (
            "gradient in a mixed batch",
            RIGHT,
            &mixed_fills,
            &mixed_full,
        ),
    ] {
        let scope = device_scope(area);
        let (mut leaked, mut painted) = (0, 0);
        // Restrict the comparison to this panel's half of the window.
        let half = if area == LEFT { 0..400 } else { 400..800 };
        for (x, y, pixel) in full.enumerate_pixels() {
            if !half.contains(&x) {
                continue;
            }
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let k = scope.rounded_coverage(p);
            let differs = pixel
                .0
                .iter()
                .zip(reference.get_pixel(x, y).0)
                .any(|(a, b)| a.abs_diff(b) > 1);
            if k == 0.0 && differs {
                leaked += 1;
            }
            if k == 1.0 && differs {
                painted += 1;
            }
        }
        assert_eq!(leaked, 0, "{name}: text or image leaked outside the curve");
        assert!(
            painted > 200,
            "{name}: text and image must be visible inside the curve"
        );
        println!("{name}: text and image rounded ({painted} pixels inside, none outside)");
    }
    println!("rounded_clip_plain_solid: ok");
}
