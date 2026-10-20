//! FID-RND-009-1/2: vector primitives and text painted from a canvas, and canvas text equal to
//! ordinary text under a shared clip.

use crate::common::{SANS, hex, plain_run};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Bounds, Corners, IntoElement, LineCap, LineJoin, ParentElement, PathBuilder,
    PathStyle, Pixels, Point, StrokeOptions, Styled, TextAlign, Window, canvas, div, fill, font,
    point, px, size,
};

fn root() -> vespertine::Div {
    div()
        .relative()
        .size_full()
        .bg(hex(0x181A1F))
        .font_family(SANS)
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(hex(0xF0F1F3))
}

/// One shaped line in the default font settings with its line box top-left at `origin`.
fn paint_text(text: &'static str, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let run = plain_run(text, font(SANS), hex(0xF0F1F3));
    let line = window
        .text_system()
        .shape_line(text.into(), px(14.), &[run], None);
    line.paint(origin, px(20.), TextAlign::Left, None, window, cx)
        .expect("paints canvas text");
}

struct Vector;

impl Scene for Vector {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        root()
            .child(
                canvas(
                    |_, _, _| (),
                    |_, _, window, cx| {
                        // Open polyline, 2 px wide, round joins and caps.
                        let mut stroke = PathBuilder::stroke(px(2.)).with_style(PathStyle::Stroke(
                            StrokeOptions::default()
                                .with_line_width(2.0)
                                .with_line_cap(LineCap::Round)
                                .with_line_join(LineJoin::Round),
                        ));
                        stroke.move_to(point(px(80.), px(120.)));
                        stroke.line_to(point(px(160.), px(60.)));
                        stroke.line_to(point(px(240.), px(120.)));
                        window.paint_path(stroke.build().expect("polyline"), hex(0x7DCFFF));

                        // Closed filled triangle.
                        let mut triangle = PathBuilder::fill();
                        triangle.add_polygon(
                            &[
                                point(px(320.), px(120.)),
                                point(px(400.), px(60.)),
                                point(px(480.), px(120.)),
                            ],
                            true,
                        );
                        window.paint_path(triangle.build().expect("triangle"), hex(0xD0A060));

                        // Circle of radius 30 at (600, 90): a 60 x 60 quad with radii 30.
                        window.paint_quad(
                            fill(
                                Bounds {
                                    origin: point(px(570.), px(60.)),
                                    size: size(px(60.), px(60.)),
                                },
                                hex(0x80C090),
                            )
                            .corner_radii(Corners::all(px(30.))),
                        );

                        paint_text("Canvas text", point(px(80.), px(180.)), window, cx);
                    },
                )
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full(),
            )
            .into_any_element()
    }
}

const CLIPPED_TEXT: &str = "Canvas and ordinary text should match exactly";

fn panel(left: f32) -> vespertine::Div {
    div()
        .absolute()
        .left(px(left))
        .top(px(300.))
        .w(px(300.))
        .h(px(60.))
        .bg(hex(0x30343D))
        .rounded(px(12.))
        .overflow_hidden()
}

struct TextClip;

impl Scene for TextClip {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        root()
            .child(
                panel(80.).child(
                    div()
                        .absolute()
                        .left(px(16.))
                        .top(px(20.))
                        .whitespace_nowrap()
                        .child(CLIPPED_TEXT),
                ),
            )
            .child(
                panel(420.).child(
                    canvas(
                        |_, _, _| (),
                        |bounds, _, window, cx| {
                            paint_text(
                                CLIPPED_TEXT,
                                bounds.origin + point(px(16.), px(20.)),
                                window,
                                cx,
                            );
                        },
                    )
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .w(px(300.))
                    .h(px(60.)),
                ),
            )
            .into_any_element()
    }
}

register_scene!("FID-RND-009-1", || Vector);
register_scene!("FID-RND-009-2", || TextClip);
