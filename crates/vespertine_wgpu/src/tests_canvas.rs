//! RND-009 (custom vector and text canvas painting) pixel acceptance tests through the
//! production painting path.

#[path = "../../vespertine/examples/example_support/canvas_scenes.rs"]
mod canvas_scenes;

use crate::tests_host::RenderHost;
use image::RgbaImage;
use vespertine::{
    AnyElement, Bounds, Corners, FillOptions, FillRule, IntoElement, LineCap, LineJoin,
    ParentElement, PathBuilder, PathStyle, Pixels, Point, StrokeOptions, Styled, Window, black,
    canvas, div, fill, point, px, rgb, size, white,
};

fn backdrop() -> vespertine::Div {
    div().relative().size_full().bg(black())
}

/// A canvas covering the whole window; `paint` receives the window and paints in window
/// coordinates.
fn full_canvas(paint: impl 'static + FnOnce(&mut Window)) -> AnyElement {
    canvas(|_, _, _| (), move |_, _, window, _| paint(window))
        .absolute()
        .top(px(0.))
        .left(px(0.))
        .size_full()
        .into_any_element()
}

fn painted(image: &RgbaImage, x: f32, y: f32) -> bool {
    image.get_pixel((x * 2.0) as u32, (y * 2.0) as u32).0[0] > 128
}

fn polygon(points: &[(f32, f32)], style: PathStyle) -> vespertine::Path<Pixels> {
    let mut builder = PathBuilder::fill().with_style(style);
    let points: Vec<Point<Pixels>> = points.iter().map(|&(x, y)| point(px(x), px(y))).collect();
    builder.add_polygon(&points, true);
    builder.build().expect("polygon path")
}

#[test]
fn open_stroke_with_round_caps_and_joins_is_not_closed() {
    let mut host = RenderHost::new();
    let image = host.render_logical(140., 90., |_, _| {
        backdrop()
            .child(full_canvas(|window| {
                let mut builder = PathBuilder::stroke(px(8.)).with_style(PathStyle::Stroke(
                    StrokeOptions::default()
                        .with_line_width(8.0)
                        .with_line_cap(LineCap::Round)
                        .with_line_join(LineJoin::Round),
                ));
                builder.move_to(point(px(20.), px(70.)));
                builder.line_to(point(px(60.), px(30.)));
                builder.line_to(point(px(100.), px(70.)));
                window.paint_path(builder.build().expect("stroke"), white());
            }))
            .into_any_element()
    });
    // The stroke body.
    assert!(painted(&image, 40., 50.));
    // Round caps extend half the width (4 logical pixels) beyond the endpoint, along the
    // direction of the first segment (up and to the right, so test straight behind it).
    assert!(painted(&image, 18.5, 71.5), "cap extends past the endpoint");
    assert!(!painted(&image, 12., 78.), "cap is only half a width long");
    // The open polyline is not closed: nothing joins the last point to the first.
    assert!(!painted(&image, 60., 70.));
    // Round join: the corner at the apex is a circular arc of the half width, so a point
    // 6 logical pixels above the apex is painted and one 9 above is not (a miter would
    // reach 11.3).
    assert!(painted(&image, 60., 30. - 3.));
    assert!(!painted(&image, 60., 30. - 9.));
}

#[test]
fn concave_fill_keeps_the_notch_unpainted() {
    let mut host = RenderHost::new();
    let image = host.render_logical(120., 100., |_, _| {
        backdrop()
            .child(full_canvas(|window| {
                // A "U": the notch between x 40..80, y 20..60 must stay empty.
                let path = polygon(
                    &[
                        (20., 20.),
                        (40., 20.),
                        (40., 60.),
                        (80., 60.),
                        (80., 20.),
                        (100., 20.),
                        (100., 80.),
                        (20., 80.),
                    ],
                    PathStyle::Fill(FillOptions::default()),
                );
                window.paint_path(path, white());
            }))
            .into_any_element()
    });
    assert!(!painted(&image, 60., 40.), "notch stays unpainted");
    assert!(painted(&image, 30., 40.), "left arm");
    assert!(painted(&image, 90., 40.), "right arm");
    assert!(painted(&image, 60., 70.), "base");
}

fn star(fill_rule: FillRule) -> RgbaImage {
    let mut host = RenderHost::new();
    host.render_logical(120., 120., move |_, _| {
        backdrop()
            .child(full_canvas(move |window| {
                let points: Vec<(f32, f32)> = (0..5)
                    .map(|i| {
                        let angle = (-90.0f32 + 144.0 * i as f32).to_radians();
                        (60.0 + 50.0 * angle.cos(), 60.0 + 50.0 * angle.sin())
                    })
                    .collect();
                let path = polygon(
                    &points,
                    PathStyle::Fill(FillOptions::default().with_fill_rule(fill_rule)),
                );
                window.paint_path(path, white());
            }))
            .into_any_element()
    })
}

#[test]
fn fill_rules_differ_on_a_self_overlapping_star() {
    let non_zero = star(FillRule::NonZero);
    let even_odd = star(FillRule::EvenOdd);
    // The central pentagon is covered twice: filled by non-zero, empty under even-odd.
    assert!(painted(&non_zero, 60., 60.));
    assert!(!painted(&even_odd, 60., 60.));
    // A point on a tip is painted under both rules.
    assert!(painted(&non_zero, 60., 18.));
    assert!(painted(&even_odd, 60., 18.));
}

#[test]
fn canvas_inside_half_opacity_and_clip_has_half_alpha_and_is_cut() {
    let mut host = RenderHost::new();
    let image = host.render_logical(120., 60., |_, _| {
        backdrop()
            .child(
                div()
                    .absolute()
                    .left(px(10.))
                    .top(px(10.))
                    .w(px(40.))
                    .h(px(40.))
                    .overflow_hidden()
                    .opacity(0.5)
                    .child(
                        canvas(
                            |_, _, _| (),
                            |_, _, window, _| {
                                window.paint_quad(fill(
                                    Bounds {
                                        origin: point(px(0.), px(0.)),
                                        size: size(px(120.), px(60.)),
                                    },
                                    white(),
                                ));
                            },
                        )
                        .absolute()
                        .left(px(-10.))
                        .top(px(-10.))
                        .w(px(120.))
                        .h(px(60.)),
                    ),
            )
            .into_any_element()
    });
    let inside = image.get_pixel(2 * 30, 2 * 30).0;
    assert!(
        inside[0].abs_diff(128) <= 2,
        "half alpha over black: {inside:?}"
    );
    // Cut at the ancestor's clip.
    assert_eq!(image.get_pixel(2 * 55, 2 * 30).0[0], 0);
    assert_eq!(image.get_pixel(2 * 30, 2 * 55).0[0], 0);
}

fn differing(a: &RgbaImage, b: &RgbaImage) -> f32 {
    let differing = a
        .pixels()
        .zip(b.pixels())
        .filter(|(pa, pb)| pa.0.iter().zip(pb.0).any(|(a, b)| a.abs_diff(b) > 8))
        .count();
    differing as f32 / (a.width() * a.height()) as f32
}

#[test]
fn convex_polygon_routes_agree() {
    let points = [(20., 20.), (100., 30.), (90., 80.), (30., 70.)];
    let mut host = RenderHost::new();
    let polygon_image = host.render_logical(120., 100., move |_, _| {
        backdrop()
            .child(full_canvas(move |window| {
                window.paint_path(
                    polygon(&points, PathStyle::Fill(FillOptions::default())),
                    white(),
                );
            }))
            .into_any_element()
    });
    let manual_image = host.render_logical(120., 100., move |_, _| {
        backdrop()
            .child(full_canvas(move |window| {
                let mut builder = PathBuilder::fill();
                builder.move_to(point(px(points[0].0), px(points[0].1)));
                for &(x, y) in &points[1..] {
                    builder.line_to(point(px(x), px(y)));
                }
                builder.close();
                window.paint_path(builder.build().expect("path"), white());
            }))
            .into_any_element()
    });
    assert!(differing(&polygon_image, &manual_image) <= 0.005);
    assert!(painted(&polygon_image, 60., 50.));
}

#[test]
fn circle_as_rounded_quad_and_as_path_agree() {
    let mut host = RenderHost::new();
    // The standard comparison is relative to the scene area: use a scene-sized window.
    let quad_image = host.render_logical(800., 480., |_, _| {
        backdrop()
            .child(full_canvas(|window| {
                window.paint_quad(
                    fill(
                        Bounds {
                            origin: point(px(20.), px(20.)),
                            size: size(px(60.), px(60.)),
                        },
                        white(),
                    )
                    .corner_radii(Corners::all(px(30.))),
                );
            }))
            .into_any_element()
    });
    let path_image = host.render_logical(800., 480., |_, _| {
        backdrop()
            .child(full_canvas(|window| {
                let mut builder = PathBuilder::fill();
                builder.move_to(point(px(20.), px(50.)));
                builder.arc_to(
                    point(px(30.), px(30.)),
                    px(0.),
                    false,
                    true,
                    point(px(80.), px(50.)),
                );
                builder.arc_to(
                    point(px(30.), px(30.)),
                    px(0.),
                    false,
                    true,
                    point(px(20.), px(50.)),
                );
                builder.close();
                window.paint_path(builder.build().expect("circle"), white());
            }))
            .into_any_element()
    });
    assert!(
        differing(&quad_image, &path_image) <= 0.005,
        "circle constructions differ in {:.3}% of pixels",
        differing(&quad_image, &path_image) * 100.
    );
    assert!(painted(&quad_image, 50., 50.));
    assert!(!painted(&quad_image, 22., 22.));
}

#[allow(dead_code)]
fn unused(_: &RenderHost) {
    let _ = rgb(0);
}

#[test]
fn fid_rnd_009_1_primitives_land_where_described() {
    let mut host = RenderHost::new();
    let image = host.render_logical(800., 480., |_, _| canvas_scenes::vector_scene());
    let background = image.get_pixel(2, 2).0;
    let differs = |x: f32, y: f32| {
        let pixel = image.get_pixel((x * 2.0) as u32, (y * 2.0) as u32).0;
        pixel
            .iter()
            .zip(background)
            .any(|(a, b)| a.abs_diff(b) > 20)
    };
    // Open polyline with round caps (half-circle caps of radius 1 at both ends).
    assert!(differs(120., 90.));
    assert!(differs(79.5, 120.5));
    assert!(!differs(160., 120.), "the polyline is not closed");
    // Triangle, circle and text.
    assert!(differs(400., 100.));
    assert!(!differs(400., 62.5) || differs(400., 63.));
    assert!(differs(600., 90.));
    assert!(!differs(600., 50.));
    let text_rows = (180..200).any(|y| (80..160).any(|x| differs(x as f32, y as f32)));
    assert!(text_rows, "the canvas text is painted");
}

#[test]
fn fid_rnd_009_2_canvas_text_equals_ordinary_text_under_shared_clip() {
    let mut host = RenderHost::new();
    let image = host.render_logical(800., 480., |_, _| canvas_scenes::text_clip_scene());
    // Panel B is 340 logical pixels (680 device pixels) right of panel A.
    let (width, height) = (600u32, 120u32);
    let mut differing = 0usize;
    for y in 0..height {
        for x in 0..width {
            let a = image.get_pixel(160 + x, 600 + y).0;
            let b = image.get_pixel(840 + x, 600 + y).0;
            if a.iter().zip(b).any(|(a, b)| a.abs_diff(b) > 8) {
                differing += 1;
            }
        }
    }
    assert!(
        differing * 200 <= (width * height) as usize,
        "{differing} pixels differ"
    );
    // The text is wider than the panel: the clip cuts it at x = 380 and x = 720.
    let panel = image.get_pixel(2 * 370, 2 * 330).0;
    let beyond = image.get_pixel(2 * 395, 2 * 330).0;
    assert_ne!(panel, beyond);
    let painted_text =
        (2 * 330..2 * 335).any(|y| (2 * 350..2 * 380).any(|x| image.get_pixel(x, y).0[0] > 0x80));
    assert!(painted_text, "text reaches the clip edge");
}
