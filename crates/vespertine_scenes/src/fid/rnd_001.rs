//! FID-RND-001-1/2/3: scoped edge fade (asymmetric four-edge fade, fade plus rounded scope
//! with double attenuation on a rounded quad, whole-subtree fade over a checkerboard).

use std::sync::Arc;

use crate::common::{SANS, hex, plain_run};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Bounds, ContentMask, Corners, EdgeFade, Hsla, IntoElement, Pixels,
    RenderImage, Styled, TextAlign, Window, canvas, fill, font, point, px, size,
};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

/// Default framework line height: 1.618034 times the font size.
const DEFAULT_LINE_HEIGHT: f32 = 1.618_034;

/// Paints a single non-wrapping, top-aligned text line whose box has its top-left at
/// `(left, top)` and the given line height.
#[allow(clippy::too_many_arguments)]
fn text_at(
    window: &mut Window,
    cx: &mut App,
    text: &'static str,
    size_px: f32,
    line_height: f32,
    color: Hsla,
    left: f32,
    top: f32,
) {
    let run = plain_run(text, font(SANS), color);
    let line = window
        .text_system()
        .shape_line(text.into(), px(size_px), &[run], None);
    line.paint(
        point(px(left), px(top)),
        px(line_height),
        TextAlign::Left,
        None,
        window,
        cx,
    )
    .ok();
}

fn mask(area: Bounds<Pixels>) -> Option<ContentMask<Pixels>> {
    Some(ContentMask { bounds: area })
}

fn all_edges(area: Bounds<Pixels>, band: f32) -> EdgeFade {
    let mut fade = EdgeFade::new(area, px(band));
    fade.top = true;
    fade.bottom = true;
    fade.left = true;
    fade.right = true;
    fade
}

struct Asymmetric;

impl Scene for Asymmetric {
    fn size(&self) -> (f32, f32) {
        (640.0, 400.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 640., 400.), hex(0x182430)));
                let area = bounds(40., 40., 560., 320.);
                let mut fade = all_edges(area, 40.);
                fade.band_top = Some(px(20.));
                fade.band_bottom = Some(px(60.));
                fade.band_left = Some(px(40.));
                fade.band_right = Some(px(80.));
                window.with_edge_fade(Some(fade), |window| {
                    window.with_content_mask(mask(area), |window| {
                        window.paint_quad(fill(area, hex(0xFFFFFF)));
                        text_at(
                            window,
                            cx,
                            "Fading content",
                            32.,
                            32. * DEFAULT_LINE_HEIGHT,
                            hex(0x203040),
                            80.,
                            170.,
                        );
                    })
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

struct RoundedDouble;

impl Scene for RoundedDouble {
    fn size(&self) -> (f32, f32) {
        (900.0, 500.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 900., 500.), hex(0x15191F)));
                let area = bounds(40., 40., 800., 420.);
                window.with_edge_fade(Some(all_edges(area, 48.)), |window| {
                    window.with_rounded_clip(area, px(40.), |window| {
                        window.paint_quad(fill(bounds(20., 20., 860., 460.), hex(0x7BC9F4)));
                        text_at(
                            window,
                            cx,
                            "Per-pixel edge coverage",
                            32.,
                            32. * DEFAULT_LINE_HEIGHT,
                            hex(0xFFFFFF),
                            60.,
                            80.,
                        );
                        window.paint_quad(
                            fill(bounds(60., 180., 760., 100.), hex(0xFFB95B))
                                .corner_radii(Corners::all(px(30.)))
                                .border_widths(px(2.))
                                .border_color(hex(0xF45869)),
                        );
                    })
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

const IMAGE_WIDTH: u32 = 600;
const IMAGE_HEIGHT: u32 = 280;

struct Checkerboard {
    image: Arc<RenderImage>,
}

impl Checkerboard {
    fn new() -> Self {
        // One uniform opaque #D0A060, as BGRA bytes.
        let pixel = [0x60, 0xA0, 0xD0, 0xFF];
        let bytes = pixel.repeat((IMAGE_WIDTH * IMAGE_HEIGHT) as usize);
        Self {
            image: Arc::new(
                RenderImage::from_bgra(IMAGE_WIDTH, IMAGE_HEIGHT, bytes)
                    .expect("valid image dimensions"),
            ),
        }
    }
}

impl Scene for Checkerboard {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let image = self.image.clone();
        canvas(
            |_, _, _| (),
            move |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 800., 480.), hex(0x181A1F)));
                for row in 0..16 {
                    for column in 0..32 {
                        let color = if (column + row) % 2 == 0 {
                            hex(0x30343D)
                        } else {
                            hex(0x586070)
                        };
                        window.paint_quad(fill(
                            bounds(80. + column as f32 * 20., 60. + row as f32 * 20., 20., 20.),
                            color,
                        ));
                    }
                }
                let area = bounds(100., 80., 600., 280.);
                window.with_edge_fade(Some(all_edges(area, 40.)), |window| {
                    window
                        .paint_image(area, area, Corners::default(), image.clone(), 0, false)
                        .ok();
                    text_at(window, cx, "Fade", 48., 60., hex(0xFFFFFF), 120., 100.);
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

register_scene!("FID-RND-001-1", || Asymmetric);
register_scene!("FID-RND-001-2", || RoundedDouble);
register_scene!("FID-RND-001-3", Checkerboard::new);
