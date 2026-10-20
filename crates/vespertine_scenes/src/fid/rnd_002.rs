//! FID-RND-002-1..5: scoped antialiased rounded content clipping (plain solid fill alone in
//! its batch, rounded scope over a window background, nested replacement, opt-in curved
//! overflow clipping with independent corners, plain solid fill sharing a batch with a
//! gradient quad).

use crate::common::{plain_run, SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Background, Bounds, Hsla, IntoElement, ParentElement, Pixels, Styled,
    TextAlign, Window, canvas, div, fill, font, linear_color_stop, linear_gradient, point,
    px, size,
};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

/// Default framework line height: 1.618034 times the font size.
const DEFAULT_LINE_HEIGHT: f32 = 1.618_034;

/// Two-stop vertical gradient in sRGB space: `top` at the top edge, `bottom` at the bottom.
fn vertical_gradient(top: u32, bottom: u32) -> Background {
    linear_gradient(
        180.,
        linear_color_stop(hex(top), 0.),
        linear_color_stop(hex(bottom), 1.),
    )
}

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

/// The two label lines of the 800 x 400 scenes, with the panel's left edge at `panel_left`.
fn panel_labels(window: &mut Window, cx: &mut App, panel_left: f32) {
    let ink = hex(0x102030);
    text_at(
        window,
        cx,
        "Rounded clip",
        36.,
        44.,
        ink,
        panel_left + 20.,
        70.,
    );
    text_at(
        window,
        cx,
        "Across corners",
        36.,
        44.,
        ink,
        panel_left + 4.,
        318.,
    );
}

/// A window-sized solid quad in a rounded scope; the scope's rectangular mask cuts it.
fn solid_panel(window: &mut Window, cx: &mut App, area: Bounds<Pixels>) {
    let left: f32 = area.origin.x.into();
    window.with_rounded_clip(area, px(36.), |window| {
        window.paint_quad(fill(bounds(0., 0., 800., 400.), hex(0xE0A040)));
        panel_labels(window, cx, left);
    });
}

struct SolidAlone;

impl Scene for SolidAlone {
    fn size(&self) -> (f32, f32) {
        (800.0, 400.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 800., 400.), hex(0x123044)));
                solid_panel(window, cx, bounds(410., 40., 360., 320.));
            },
        )
        .size_full()
        .into_any_element()
    }
}

struct SolidWithGradient;

impl Scene for SolidWithGradient {
    fn size(&self) -> (f32, f32) {
        (800.0, 400.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 800., 400.), hex(0x123044)));
                window.with_rounded_clip(bounds(30., 40., 360., 320.), px(36.), |window| {
                    window.paint_quad(fill(
                        bounds(0., 0., 800., 400.),
                        vertical_gradient(0xE0A040, 0xC06848),
                    ));
                    panel_labels(window, cx, 30.);
                });
                solid_panel(window, cx, bounds(410., 40., 360., 320.));
            },
        )
        .size_full()
        .into_any_element()
    }
}

struct OverBackground;

impl Scene for OverBackground {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 800., 480.), hex(0x181A1F)));
                window.with_rounded_clip(bounds(160., 80., 480., 300.), px(40.), |window| {
                    window.paint_quad(fill(
                        bounds(160., 80., 480., 300.),
                        vertical_gradient(0x406090, 0x304870),
                    ));
                    window.paint_quad(fill(
                        bounds(140., 60., 520., 100.),
                        vertical_gradient(0xD08040, 0xE4A060),
                    ));
                    text_at(
                        window,
                        cx,
                        "Clipped text",
                        48.,
                        60.,
                        hex(0xF0F1F3),
                        140.,
                        100.,
                    );
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

struct Nested;

impl Scene for Nested {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        canvas(
            |_, _, _| (),
            |_, _, window, cx| {
                window.paint_quad(fill(bounds(0., 0., 800., 480.), hex(0x181A1F)));
                window.with_rounded_clip(bounds(60., 60., 680., 360.), px(100.), |window| {
                    window.paint_quad(fill(
                        bounds(60., 60., 680., 360.),
                        vertical_gradient(0x3A6EA5, 0x1F3B63),
                    ));
                    window.with_rounded_clip(bounds(60., 60., 300., 200.), px(12.), |window| {
                        window.paint_quad(fill(
                            bounds(0., 0., 400., 300.),
                            vertical_gradient(0xF0A050, 0xC05040),
                        ));
                    });
                    text_at(
                        window,
                        cx,
                        "Nested",
                        32.,
                        32. * DEFAULT_LINE_HEIGHT,
                        hex(0xF0F1F3),
                        420.,
                        200.,
                    );
                });
            },
        )
        .size_full()
        .into_any_element()
    }
}

struct CurvedOverflow;

impl Scene for CurvedOverflow {
    fn size(&self) -> (f32, f32) {
        (700.0, 420.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        // Absolute children are placed relative to their parent's padding box, so every
        // offset below is the window coordinate minus the parent's padding-box origin.
        let label = div()
            .absolute()
            .left(px(24.))
            .top(px(44.))
            .text_size(px(20.))
            .text_color(hex(0xFFFFFF))
            .child("Independent corners");
        let red = div()
            .absolute()
            .left(px(-26.))
            .top(px(-26.))
            .w(px(500.))
            .h(px(260.))
            .bg(hex(0xE53A40));
        let inner = div()
            .absolute()
            .left(px(72.))
            .top(px(36.))
            .w(px(400.))
            .h(px(230.))
            .bg(hex(0x2C6DE8))
            .rounded(px(12.))
            .border(px(6.))
            .border_color(hex(0x163878))
            .overflow_hidden()
            .curved_overflow_clip()
            .child(red)
            .child(label);
        let outer = div()
            .absolute()
            .left(px(40.))
            .top(px(40.))
            .w(px(420.))
            .h(px(280.))
            .bg(hex(0xFFFFFF))
            .rounded(px(40.))
            .border_t(px(4.))
            .border_r(px(12.))
            .border_b(px(20.))
            .border_l(px(8.))
            .border_color(hex(0x202020))
            .overflow_hidden()
            .curved_overflow_clip()
            .child(inner);
        div()
            .relative()
            .size_full()
            .bg(hex(0xE8E8E8))
            .font_family(SANS)
            .child(outer)
            .into_any_element()
    }
}

register_scene!("FID-RND-002-1", || SolidAlone);
register_scene!("FID-RND-002-2", || OverBackground);
register_scene!("FID-RND-002-3", || Nested);
register_scene!("FID-RND-002-4", || CurvedOverflow);
register_scene!("FID-RND-002-5", || SolidWithGradient);
