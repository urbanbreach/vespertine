//! FID-RND-008: the same layered surface plate as a styled element (left) and painted from a
//! canvas in the canonical layer order (right).

use crate::common::{plain_run, SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Background, BorderStyle, BoxShadow, Corners, Edges, Hsla, IntoElement,
    ParentElement, Styled, TextAlign, Window, black, canvas, div, font, linear_color_stop,
    linear_gradient, point, px, quad, white,
};

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

fn plate_fill() -> Background {
    linear_gradient(
        180.,
        linear_color_stop(hex(0x505866), 0.),
        linear_color_stop(hex(0x303640), 1.),
    )
}

/// The left plate: one styled element carrying every layer.
fn styled_plate(left: f32, top: f32) -> AnyElement {
    div()
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(240.))
        .h(px(100.))
        .rounded(px(12.))
        .bg(plate_fill())
        .border_1()
        .border_color(hex(0x8993A3))
        .shadow(vec![drop_shadow(), inset_shadow()])
        .flex()
        .items_center()
        .justify_center()
        .child("Plate")
        .into_any_element()
}

/// The right plate: a canvas painting drop shadow, fill, inset shadow, text, then the
/// border-only quad.
fn canvas_plate(left: f32, top: f32) -> AnyElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, cx| {
            let radii = Corners::all(px(12.));
            let shadows = [drop_shadow(), inset_shadow()];
            window.paint_drop_shadows(bounds, radii, &shadows);
            window.paint_quad(quad(
                bounds,
                radii,
                plate_fill(),
                Edges::default(),
                Hsla {
                    a: 0.,
                    ..hex(0x505866)
                },
                BorderStyle::default(),
            ));
            window.paint_inset_shadows(bounds, radii, &shadows);
            let run = plain_run("Plate", font(SANS), hex(0xF0F1F3));
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
            .expect("paints text");
            window.paint_quad(quad(
                bounds,
                radii,
                Hsla {
                    a: 0.,
                    ..hex(0x8993A3)
                },
                Edges::all(px(1.)),
                hex(0x8993A3),
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

struct Plates;

impl Scene for Plates {
    fn size(&self) -> (f32, f32) {
        (800.0, 480.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .relative()
            .size_full()
            .bg(hex(0x181A1F))
            .font_family(SANS)
            .text_size(px(14.))
            .line_height(px(20.))
            .text_color(hex(0xF0F1F3))
            .child(styled_plate(80., 80.))
            .child(canvas_plate(420., 80.))
            .into_any_element()
    }
}

register_scene!("FID-RND-008", || Plates);
