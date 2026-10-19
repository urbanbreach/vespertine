//! The FID-RND-009 scenes: vector primitives and text painted from a canvas, and canvas text
//! compared with ordinary text under a shared clip. Uses only public API; shared by the
//! canvas example and the renderer tests.
#![allow(dead_code)]

use vespertine::{
    AnyElement, App, Bounds, Corners, IntoElement, LineCap, LineJoin, ParentElement, PathBuilder,
    PathStyle, Pixels, Point, SharedString, StrokeOptions, Styled, TextAlign, TextRun, Window,
    canvas, div, fill, hsla, point, px, rgb, size,
};

pub const BACKGROUND: u32 = 0x181A1F;

pub fn scene_root() -> vespertine::Div {
    div()
        .relative()
        .size_full()
        .bg(rgb(BACKGROUND))
        .font_family("Noto Sans")
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(rgb(0xF0F1F3))
}

fn paint_text(text: &str, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let style = window.text_style();
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color: rgb(0xF0F1F3).into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(14.),
        &[run],
        None,
    );
    line.paint(origin, px(20.), TextAlign::Left, None, window, cx)
        .expect("paint canvas text");
}

/// FID-RND-009-1: vector primitives and text from one canvas (window 800 x 480).
pub fn vector_scene() -> AnyElement {
    scene_root()
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
                    window.paint_path(stroke.build().expect("polyline"), rgb(0x7DCFFF));

                    // Filled triangle.
                    let mut triangle = PathBuilder::fill();
                    triangle.add_polygon(
                        &[
                            point(px(320.), px(120.)),
                            point(px(400.), px(60.)),
                            point(px(480.), px(120.)),
                        ],
                        true,
                    );
                    window.paint_path(triangle.build().expect("triangle"), rgb(0xD0A060));

                    // Filled circle of radius 30 centered at (600, 90).
                    window.paint_quad(
                        fill(
                            Bounds {
                                origin: point(px(570.), px(60.)),
                                size: size(px(60.), px(60.)),
                            },
                            rgb(0x80C090),
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

const CLIPPED_TEXT: &str = "Canvas and ordinary text should match exactly";

fn panel(left: f32) -> vespertine::Div {
    div()
        .absolute()
        .left(px(left))
        .top(px(300.))
        .w(px(300.))
        .h(px(60.))
        .bg(rgb(0x30343D))
        .rounded(px(12.))
        .overflow_hidden()
}

/// FID-RND-009-2: panel A (ordinary text) and panel B (canvas text) under the same clip
/// (window 800 x 480). Panel B is 340 logical pixels to the right of panel A.
pub fn text_clip_scene() -> AnyElement {
    scene_root()
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

#[allow(unused)]
fn unused() {
    let _ = hsla(0., 0., 0., 0.);
}
