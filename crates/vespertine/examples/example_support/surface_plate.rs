//! The FID-RND-008 plate scene: one styled element and the same surface painted from a
//! canvas in the canonical layer order (drop shadows, fill, inset shadows, content, border).
//! Shared by the surface plate example and the renderer tests; uses only public API.
#![allow(dead_code)]

use vespertine::{
    AnyElement, BorderStyle, BoxShadow, Corners, Edges, Hsla, IntoElement, ParentElement, Styled,
    TextAlign, TextRun, black, canvas, div, linear_color_stop, linear_gradient, point, px, quad,
    rgb, white,
};

pub const BACKGROUND: u32 = 0x181A1F;

fn drop_shadow() -> BoxShadow {
    BoxShadow {
        color: Hsla {
            a: f32::from(0x66u8) / 255.0,
            ..black()
        },
        offset: point(px(0.), px(6.)),
        blur_radius: px(12.),
        spread_radius: px(0.),
        inset: false,
    }
}

fn inset_shadow() -> BoxShadow {
    BoxShadow {
        color: Hsla {
            a: f32::from(0x33u8) / 255.0,
            ..white()
        },
        offset: point(px(0.), px(1.)),
        blur_radius: px(0.),
        spread_radius: px(0.),
        inset: true,
    }
}

fn plate_fill() -> vespertine::Background {
    linear_gradient(
        180.,
        linear_color_stop(rgb(0x505866), 0.),
        linear_color_stop(rgb(0x303640), 1.),
    )
}

/// The left plate of FID-RND-008: a single styled element carrying every layer.
pub fn styled_plate(left: f32, top: f32) -> AnyElement {
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(240.))
        .h(px(100.))
        .rounded(px(12.))
        .bg(plate_fill())
        .border_1()
        .border_color(rgb(0x8993A3))
        .shadow(vec![drop_shadow(), inset_shadow()])
        .flex()
        .items_center()
        .justify_center()
        .child("Plate")
        .into_any_element()
}

/// The right plate of FID-RND-008: a canvas painting the canonical layer order.
pub fn canvas_plate(left: f32, top: f32) -> AnyElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, cx| {
            let radii = Corners::all(px(12.));
            let shadows = [drop_shadow(), inset_shadow()];
            window.paint_drop_shadows(bounds, radii, &shadows);
            let transparent_border = Hsla {
                a: 0.,
                ..Hsla::from(rgb(0x8993A3))
            };
            window.paint_quad(quad(
                bounds,
                radii,
                plate_fill(),
                Edges::default(),
                Hsla {
                    a: 0.,
                    ..Hsla::from(rgb(0x505866))
                },
                BorderStyle::default(),
            ));
            window.paint_inset_shadows(bounds, radii, &shadows);
            let style = window.text_style();
            let run = TextRun {
                strikethrough: None,
                underline: None,
                background_color: None,
                color: rgb(0xF0F1F3).into(),
                font: style.font(),
                len: "Plate".len(),
            };
            let line = window
                .text_system()
                .shape_line("Plate".into(), px(14.), &[run], None);
            line.paint(
                point(bounds.left(), bounds.top() + px(40.)),
                px(20.),
                TextAlign::Center,
                Some(bounds.size.width),
                window,
                cx,
            )
            .expect("paint text");
            window.paint_quad(quad(
                bounds,
                radii,
                transparent_border,
                Edges::all(px(1.)),
                rgb(0x8993A3),
                BorderStyle::default(),
            ));
        },
    )
    .absolute()
    .left(px(left))
    .top(px(top))
    .w(px(240.))
    .h(px(100.))
    .into_any_element()
}

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
