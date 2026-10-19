//! Scoped rounded clipping (RND-002 A2): text, an image and a gradient-filled quad are
//! swept past each of the four corners of a rounded scope over a translucent panel. The
//! content is cut along the curved edge itself, per fragment and antialiased: no pixel outside
//! the rounded coverage changes, no corner-cover shapes are painted, and adding the wrapper
//! does not change layout.
//!
//! The example renders every frame headlessly twice, with and without the clipped content,
//! and asserts on the pixels. Run it with `--out <dir>` to also write the frames as PNGs:
//!
//! ```sh
//! cargo run -p vespertine --features test-support --example rounded_clip -- --out /tmp/clip
//! ```

#[path = "example_support/clip_host.rs"]
mod clip_host;

use clip_host::{Host, compare_with_reference, output_directory, save, scope};
use image::{Frame, ImageBuffer, Rgba};
use std::{cell::Cell, rc::Rc, sync::Arc};
use vespertine::{
    AnyElement, Bounds, Corners, IntoElement, ParentElement, Pixels, RenderImage, Styled, canvas,
    div, linear_color_stop, linear_gradient, point, px, rgb, rgba, rounded_clip, size, white,
};

const WIDTH: f32 = 400.0;
const HEIGHT: f32 = 300.0;
/// The scope and panel, in window logical pixels.
const PANEL: [f32; 4] = [40.0, 40.0, 360.0, 260.0];
const RADIUS: f32 = 32.0;

fn gradient_image() -> Arc<RenderImage> {
    // BGRA bytes: a diagonal blue to orange ramp, opaque.
    let buffer = ImageBuffer::from_fn(64, 64, |x, y| {
        let t = (x + y) as f32 / 126.0;
        Rgba([
            (200.0 * (1.0 - t) + 40.0 * t) as u8,
            (120.0 * (1.0 - t) + 160.0 * t) as u8,
            (40.0 * (1.0 - t) + 230.0 * t) as u8,
            255,
        ])
    });
    Arc::new(RenderImage::new(vec![Frame::new(buffer)]))
}

/// The three kinds of clipped content centered on `center` (window logical pixels).
fn content(center: (f32, f32)) -> Vec<AnyElement> {
    let (cx, cy) = center;
    let (left, top) = (PANEL[0], PANEL[1]);
    let image = gradient_image();
    vec![
        // Text.
        div()
            .absolute()
            .left(px(cx - 70.0 - left))
            .top(px(cy - 18.0 - top))
            .text_color(white())
            .text_size(px(28.0))
            .child("Rounded clip")
            .into_any_element(),
        // An image.
        canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let bounds = Bounds {
                    origin: point(px(cx - 32.0), px(cy - 32.0)),
                    size: size(px(64.0), px(64.0)),
                };
                window
                    .paint_image(bounds, bounds, Corners::default(), image, 0, false)
                    .expect("paint image");
            },
        )
        .absolute()
        .size_full()
        .into_any_element(),
        // A gradient-filled quad (a general-route quad, so the clip applies to it).
        div()
            .absolute()
            .left(px(cx - 40.0 - left))
            .top(px(cy - 28.0 - top))
            .w(px(80.0))
            .h(px(56.0))
            .bg(linear_gradient(
                180.0,
                linear_color_stop(rgb(0xe0a040), 0.0),
                linear_color_stop(rgb(0xc06848), 1.0),
            ))
            .into_any_element(),
    ]
}

/// The window: background, a translucent rounded panel, and (when `clipped_content` is a
/// center) the three content kinds inside a [`rounded_clip`] over the panel's bounds.
fn window(clipped_content: Option<(f32, f32)>) -> AnyElement {
    let panel_width = PANEL[2] - PANEL[0];
    let panel_height = PANEL[3] - PANEL[1];
    let mut root = div().size_full().bg(rgb(0x181a1f)).child(
        // The translucent panel: its own antialiased corners are the curve to match.
        div()
            .absolute()
            .left(px(PANEL[0]))
            .top(px(PANEL[1]))
            .w(px(panel_width))
            .h(px(panel_height))
            .rounded(px(RADIUS))
            .bg(rgba(0x5080ff55)),
    );
    if let Some(center) = clipped_content {
        root = root.child(
            div()
                .absolute()
                .left(px(PANEL[0]))
                .top(px(PANEL[1]))
                .w(px(panel_width))
                .h(px(panel_height))
                .child(rounded_clip(
                    px(RADIUS),
                    div().size_full().children(content(center)),
                )),
        );
    }
    root.into_any_element()
}

/// Bounds of a flexible item with and without the wrapper around it.
fn measured_item(wrapped: bool) -> (Bounds<Pixels>, Bounds<Pixels>) {
    let item = Rc::new(Cell::new(Bounds::default()));
    let sibling = Rc::new(Cell::new(Bounds::default()));
    let (item_cell, sibling_cell) = (item.clone(), sibling.clone());
    let mut host = Host::new();
    host.render(300.0, 80.0, move |_, _| {
        let (item_cell, sibling_cell) = (item_cell.clone(), sibling_cell.clone());
        let flexible = canvas(move |bounds, _, _| item_cell.set(bounds), |_, _, _, _| {})
            .flex_1()
            .h(px(40.0));
        let flexible: AnyElement = if wrapped {
            rounded_clip(px(12.0), flexible).into_any_element()
        } else {
            flexible.into_any_element()
        };
        div()
            .size_full()
            .flex()
            .child(div().w(px(60.0)).h(px(40.0)))
            .child(flexible)
            .child(
                canvas(
                    move |bounds, _, _| sibling_cell.set(bounds),
                    |_, _, _, _| {},
                )
                .w(px(50.0))
                .h(px(40.0)),
            )
            .into_any_element()
    });
    (item.get(), sibling.get())
}

fn main() {
    let output = output_directory();
    let mut host = Host::new();
    // The scope in device pixels (scale factor 2) for the reference coverage.
    let device_scope = scope(PANEL.map(|value| value * 2.0), RADIUS * 2.0);

    // The reference frame: everything except the clipped content.
    let reference = host.render(WIDTH, HEIGHT, |_, _| window(None));
    save(&output, "reference", &reference);

    // Sweep the content center around the panel perimeter, starting at each corner:
    // top-left, top-right, bottom-right, bottom-left, six steps per side.
    let corners = [
        (PANEL[0], PANEL[1]),
        (PANEL[2], PANEL[1]),
        (PANEL[2], PANEL[3]),
        (PANEL[0], PANEL[3]),
    ];
    let mut frame = 0;
    let (mut cut_edge_pixels, mut total_painted) = (0usize, 0usize);
    for side in 0..4 {
        let (from, to) = (corners[side], corners[(side + 1) % 4]);
        for step in 0..6 {
            let t = step as f32 / 6.0;
            let center = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            let image = host.render(WIDTH, HEIGHT, move |_, _| window(Some(center)));
            // Nothing outside the rounded coverage changes (a one-level tolerance for
            // blending rounding); and the content did paint inside the curve.
            let (leaked, painted) = compare_with_reference(&image, &reference, &device_scope, 1);
            assert_eq!(
                leaked, 0,
                "frame {frame}: {leaked} pixels leaked outside the curve"
            );
            assert!(
                painted > 500,
                "frame {frame}: the clipped content must be visible"
            );
            total_painted += painted;
            // Antialiasing: pixels on the curve itself are partially covered.
            cut_edge_pixels += image
                .enumerate_pixels()
                .filter(|(x, y, pixel)| {
                    let k = device_scope.rounded_coverage([*x as f32 + 0.5, *y as f32 + 0.5]);
                    k > 0.05 && k < 0.95 && pixel.0 != reference.get_pixel(*x, *y).0
                })
                .count();
            if step == 0 {
                save(&output, &format!("corner_{side}"), &image);
            }
            frame += 1;
        }
    }
    assert!(
        cut_edge_pixels > 200,
        "the cut must be antialiased on the curve"
    );
    println!(
        "{frame} frames: no pixel outside the rounded coverage changed; {total_painted} \
         pixels painted inside; {cut_edge_pixels} antialiased edge pixels"
    );

    // No layout change from adding the wrapper: a flexible item and its sibling measure the
    // same with and without it.
    let plain = measured_item(false);
    let wrapped = measured_item(true);
    assert_eq!(plain, wrapped, "the wrapper must not change layout");
    assert_eq!(plain.0.size.width, px(190.0));
    println!(
        "wrapper layout unchanged: item {:?}, sibling {:?}",
        plain.0, plain.1
    );
    println!("rounded_clip: ok");
}
