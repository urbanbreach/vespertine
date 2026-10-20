//! FID-RND-003-1/2/3: ordered Gaussian backdrop blur (ordering, per-corner radii, frosted
//! panel over stripes, floating card with a nested overlay).

use crate::common::{SANS, hex, plain_run};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, BorderStyle, Bounds, Corners, Hsla, IntoElement, Pixels, Styled, TextAlign,
    Window, canvas, fill, font, point, px, quad, rgba, size,
};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

fn corners(top_left: f32, top_right: f32, bottom_right: f32, bottom_left: f32) -> Corners<Pixels> {
    Corners {
        top_left: px(top_left),
        top_right: px(top_right),
        bottom_right: px(bottom_right),
        bottom_left: px(bottom_left),
    }
}

/// A blur followed by its visible tint over the same rounded bounds.
fn blur_with_tint(
    window: &mut Window,
    area: Bounds<Pixels>,
    radii: Corners<Pixels>,
    blur_radius: f32,
    tint: Hsla,
) {
    window.paint_backdrop_blur(area, radii, px(blur_radius));
    window.paint_quad(fill(area, tint).corner_radii(radii));
}

/// Paints a single shaped line with its left edge at `left` and its baseline at `baseline`.
fn text_at(
    window: &mut Window,
    cx: &mut App,
    text: &'static str,
    size_px: f32,
    color: Hsla,
    left: f32,
    baseline: f32,
) {
    let run = plain_run(text, font(SANS), color);
    let line = window
        .text_system()
        .shape_line(text.into(), px(size_px), &[run], None);
    // A line height of ascent + descent puts the baseline exactly `ascent` below the origin.
    let origin = point(px(left), px(baseline) - line.ascent);
    line.paint(
        origin,
        line.ascent + line.descent,
        TextAlign::Left,
        None,
        window,
        cx,
    )
    .ok();
}

struct Ordering;

impl Scene for Ordering {
    fn size(&self) -> (f32, f32) {
        (900.0, 500.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, _| {
                window.paint_quad(fill(bounds(0., 0., 900., 500.), hex(0x18202B)));
                for row in 0..50 {
                    for column in 0..90 {
                        if (row + column) % 2 == 0 {
                            window.paint_quad(fill(
                                bounds(column as f32 * 10., row as f32 * 10., 10., 10.),
                                hex(0xFFFFFF),
                            ));
                        }
                    }
                }
                blur_with_tint(
                    window,
                    bounds(40., 60., 350., 240.),
                    corners(24., 24., 24., 24.),
                    8.,
                    rgba(0xFFFFFF1A).into(),
                );
                window.paint_quad(fill(bounds(150., 100., 80., 30.), hex(0xE34D59)));
                blur_with_tint(
                    window,
                    bounds(290., 180., 350., 240.),
                    corners(4., 24., 40., 12.),
                    16.,
                    rgba(0x85C8FF1F).into(),
                );
                blur_with_tint(
                    window,
                    bounds(0., 360., 230., 140.),
                    corners(0., 0., 0., 0.),
                    5.5,
                    rgba(0xFFFFFF0D).into(),
                );
            },
        )
        .size_full()
        .into_any_element()
    }
}

struct Frosted;

impl Scene for Frosted {
    fn size(&self) -> (f32, f32) {
        (640.0, 400.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 640., 400.), hex(0x101820)));
                for stripe in 0..16 {
                    window.paint_quad(fill(
                        bounds(stripe as f32 * 40., 0., 20., 400.),
                        hex(0xF0C040),
                    ));
                }
                let panel = bounds(160., 80., 320., 220.);
                window.paint_backdrop_blur(panel, corners(28., 28., 28., 28.), px(16.));
                window.paint_quad(quad(
                    panel,
                    corners(28., 28., 28., 28.),
                    Hsla::transparent_black(),
                    px(1.),
                    Hsla::from(rgba(0xFFFFFF80)),
                    BorderStyle::Solid,
                ));
                text_at(window, cx, "Frosted panel", 32., hex(0xFFFFFF), 190., 200.);
            },
        )
        .size_full()
        .into_any_element()
    }
}

struct Card;

impl Scene for Card {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 800., 480.), hex(0x181A1F)));
                for stripe in 0..40 {
                    let color = if stripe % 2 == 0 { 0x202838 } else { 0x546078 };
                    window.paint_quad(fill(bounds(stripe as f32 * 20., 0., 20., 480.), hex(color)));
                }
                let card = bounds(220., 100., 360., 240.);
                let radii = corners(16., 16., 16., 16.);
                window.paint_layer(card, |window| {
                    window.paint_backdrop_blur(card, radii, px(16.));
                    window.paint_quad(fill(card, Hsla::from(rgba(0x20242CCC))).corner_radii(radii));
                    window.paint_quad(quad(
                        card,
                        radii,
                        Hsla::transparent_black(),
                        px(1.),
                        Hsla::from(rgba(0xA0A8B866)),
                        BorderStyle::Solid,
                    ));
                    text_at(
                        window,
                        cx,
                        "Sharp foreground",
                        14.,
                        hex(0xF0F1F3),
                        244.,
                        139.,
                    );
                    let overlay = bounds(456., 144., 100., 30.);
                    window.paint_layer(overlay, |window| {
                        window.paint_quad(fill(overlay, hex(0x406090)));
                        text_at(window, cx, "Overlay", 14., hex(0xF0F1F3), 464., 164.);
                    });
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

register_scene!("FID-RND-003-1", || Ordering);
register_scene!("FID-RND-003-2", || Frosted);
register_scene!("FID-RND-003-3", || Card);
