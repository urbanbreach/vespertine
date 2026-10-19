//! PRF-006 (translation-invariant tessellation cache) pixel acceptance: the pixels of a path
//! built by a fresh tessellation and of the same path built from the warm cache are identical.

use crate::tests_host::RenderHost;
use image::RgbaImage;
use vespertine::{
    FillOptions, FillRule, IntoElement, LineCap, LineJoin, ParentElement, Path, PathBuilder,
    PathStyle, Pixels, StrokeOptions, Styled, canvas, div, path_cache_stats, point, px,
    reset_path_cache, rgb, white,
};

const ORIGINS: [(f32, f32); 4] = [(10.0, 20.0), (60.5, 40.25), (-6.0, 8.5), (100.25, 61.0)];

fn at(origin: (f32, f32), x: f32, y: f32) -> vespertine::Point<Pixels> {
    point(px(origin.0 + x), px(origin.1 + y))
}

fn stroke_curve(origin: (f32, f32)) -> Path<Pixels> {
    let mut builder = PathBuilder::stroke(px(4.0)).with_style(PathStyle::Stroke(
        StrokeOptions::default()
            .with_line_width(4.0)
            .with_line_cap(LineCap::Round)
            .with_line_join(LineJoin::Round),
    ));
    builder.move_to(at(origin, 0.0, 30.0));
    builder.curve_to(at(origin, 30.0, 30.0), at(origin, 15.0, 0.0));
    builder.cubic_bezier_to(
        at(origin, 60.0, 30.0),
        at(origin, 40.0, 60.0),
        at(origin, 52.0, 0.0),
    );
    builder.build().expect("stroke builds")
}

fn filled_shape(origin: (f32, f32)) -> Path<Pixels> {
    let mut builder = PathBuilder::fill().with_style(PathStyle::Fill(
        FillOptions::default().with_fill_rule(FillRule::NonZero),
    ));
    builder.move_to(at(origin, 0.0, 0.0));
    builder.line_to(at(origin, 36.0, 4.0));
    builder.curve_to(at(origin, 44.0, 36.0), at(origin, 50.0, 12.0));
    builder.cubic_bezier_to(
        at(origin, 4.0, 44.0),
        at(origin, 30.0, 52.0),
        at(origin, 10.0, 56.0),
    );
    builder.close();
    builder.build().expect("fill builds")
}

fn paint(paths: Vec<Path<Pixels>>) -> RgbaImage {
    let mut host = RenderHost::new();
    host.render_logical(180., 130., move |_, _| {
        let paths = paths.clone();
        div()
            .relative()
            .size_full()
            .bg(rgb(0x101820))
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, _| {
                        for (i, path) in paths.iter().enumerate() {
                            let color = if i % 2 == 0 {
                                white()
                            } else {
                                rgb(0x2563eb).into()
                            };
                            window.paint_path(path.clone(), color);
                        }
                    },
                )
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .size_full(),
            )
            .into_any_element()
    })
}

#[test]
fn cached_translated_paths_paint_the_same_pixels_as_fresh_tessellations() {
    // Fresh: every origin tessellated by an empty cache.
    let fresh: Vec<RgbaImage> = ORIGINS
        .iter()
        .map(|&origin| {
            reset_path_cache();
            paint(vec![stroke_curve(origin), filled_shape(origin)])
        })
        .collect();

    // Cached: the shapes are warmed at the first origin, every other origin reuses them.
    reset_path_cache();
    let mut cached = Vec::new();
    for &origin in &ORIGINS {
        cached.push(paint(vec![stroke_curve(origin), filled_shape(origin)]));
    }
    let stats = path_cache_stats();
    assert_eq!(
        stats.tessellations, 2,
        "one stroke and one fill tessellation in total"
    );
    assert_eq!(stats.hits, 2 * (ORIGINS.len() as u64 - 1));

    for (i, (a, b)) in fresh.iter().zip(&cached).enumerate() {
        assert!(
            a.as_raw() == b.as_raw(),
            "pixels at origin {:?} differ between fresh and cached",
            ORIGINS[i]
        );
        assert!(
            a.pixels().any(|p| p.0[0] > 200 || p.0[2] > 200),
            "the shapes are visible"
        );
    }
    // The shapes really moved: different origins give different images.
    assert!(fresh[0].as_raw() != fresh[1].as_raw());
}

#[test]
fn a_changed_shape_never_reuses_stale_triangles() {
    reset_path_cache();
    let wide = {
        let mut builder = PathBuilder::stroke(px(4.0));
        builder.move_to(point(px(10.0), px(30.0)));
        builder.line_to(point(px(90.0), px(30.0)));
        builder.build().unwrap()
    };
    let wider = {
        let mut builder = PathBuilder::stroke(px(10.0));
        builder.move_to(point(px(10.0), px(60.0)));
        builder.line_to(point(px(90.0), px(60.0)));
        builder.build().unwrap()
    };
    assert_eq!(path_cache_stats().tessellations, 2);
    let image = paint(vec![wide, wider]);
    let painted = |x: f32, y: f32| image.get_pixel((x * 2.0) as u32, (y * 2.0) as u32).0[2] > 128;
    assert!(painted(50.0, 31.5) && !painted(50.0, 34.0), "4 px line");
    assert!(painted(50.0, 64.0) && !painted(50.0, 66.5), "10 px line");
}
