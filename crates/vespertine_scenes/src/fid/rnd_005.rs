//! FID-RND-005-1/2: composite-time transformed and blurred glyph painting (a rotated and a
//! blurred "A"; blurred and rotated bold text inside a rounded clip with a right edge fade).

use crate::common::{SANS, hex, plain_run};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Bounds, EdgeFade, FontId, GlyphId, Hsla, IntoElement, ParentElement, Pixels,
    Point, ScaledPixels, Styled, TransformationMatrix, Window, canvas, div, font, point, px,
    radians, size,
};

/// Rotation by `degrees` clockwise about the device-pixel `pivot` (t = pivot - M pivot).
fn rotation_about(pivot: (f32, f32), degrees: f32) -> TransformationMatrix {
    TransformationMatrix::unit()
        .translate(point(ScaledPixels(pivot.0), ScaledPixels(pivot.1)))
        .rotate(radians(degrees.to_radians()))
        .translate(point(ScaledPixels(-pivot.0), ScaledPixels(-pivot.1)))
}

type Glyphs = Vec<(FontId, GlyphId, Point<Pixels>)>;

/// Shapes `text` and returns each glyph's font, id and position relative to the baseline origin.
fn shape(window: &mut Window, text: &str, bold: bool, size: f32) -> Glyphs {
    let mut font = font(SANS);
    if bold {
        font = font.bold();
    }
    let run = plain_run(text, font, hex(0xFFFFFF));
    let line = window
        .text_system()
        .shape_line(text.to_string().into(), px(size), &[run], None);
    line.runs
        .iter()
        .flat_map(|run| {
            run.glyphs
                .iter()
                .map(|glyph| (run.font_id, glyph.id, glyph.position))
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn paint_glyphs(
    window: &mut Window,
    glyphs: &Glyphs,
    origin: Point<Pixels>,
    font_size: f32,
    color: Hsla,
    transformation: TransformationMatrix,
    blur: f32,
) {
    for (font_id, glyph_id, position) in glyphs {
        window
            .paint_glyph_transformed(
                origin + *position,
                *font_id,
                *glyph_id,
                px(font_size),
                color,
                transformation,
                px(blur),
            )
            .expect("glyph paints");
    }
}

struct Single;

impl Scene for Single {
    fn size(&self) -> (f32, f32) {
        (600.0, 360.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .size_full()
            .bg(hex(0x202838))
            .child(
                canvas(
                    |_, _, _| (),
                    |_, _, window, _| {
                        let color = hex(0xF0D080);
                        let glyphs = shape(window, "A", false, 80.0);
                        let unit = TransformationMatrix::unit();
                        for (x, transformation, blur) in [
                            (70.0, unit, 0.0),
                            (250.0, rotation_about((500.0, 420.0), 15.0), 0.0),
                            (430.0, unit, 4.0),
                        ] {
                            paint_glyphs(
                                window,
                                &glyphs,
                                point(px(x), px(210.0)),
                                80.0,
                                color,
                                transformation,
                                blur,
                            );
                        }
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

struct Scoped;

impl Scene for Scoped {
    fn size(&self) -> (f32, f32) {
        (600.0, 500.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .size_full()
            .bg(hex(0x111823))
            .child(
                canvas(
                    |_, _, _| (),
                    |_, _, window, _| {
                        let color = hex(0xE9F4FF);
                        let glyphs = shape(window, "Blurred glyphs 0123", true, 40.0);
                        let region = Bounds {
                            origin: point(px(40.0), px(30.0)),
                            size: size(px(420.0), px(430.0)),
                        };
                        let mut fade = EdgeFade::new(region, px(48.0));
                        fade.right = true;
                        window.with_rounded_clip(region, px(24.0), |window| {
                            window.with_edge_fade(Some(fade), |window| {
                                for (top, blur, degrees) in
                                    [(50.0, 0.0, 0.0), (190.0, 1.0, 8.0), (330.0, 2.5, 0.0)]
                                {
                                    let transformation = if degrees == 0.0 {
                                        TransformationMatrix::unit()
                                    } else {
                                        rotation_about((540.0, 460.0), degrees)
                                    };
                                    paint_glyphs(
                                        window,
                                        &glyphs,
                                        point(px(60.0), px(top + 40.0)),
                                        40.0,
                                        color,
                                        transformation,
                                        blur,
                                    );
                                }
                            })
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}

register_scene!("FID-RND-005-1", || Single);
register_scene!("FID-RND-005-2", || Scoped);
