//! Production-path pixel tests of the rounded clip scope, its wrapper and the opt-in curved
//! overflow clip (RND-002): A4, A6 (as a test), A7, A9 and the R11 and R15 behaviors, through
//! real element trees, real text and the wgpu renderer at scale factors 1 and 2.

use crate::tests_host::RenderHost;
use image::{Frame, ImageBuffer, Rgba, RgbaImage};
use std::sync::Arc;
use vespertine::{
    AnyElement, Bounds, BoxShadow, Corners, CurvedClipShape, IntoElement, ParentElement,
    PathBuilder, RenderImage, ScopeParams, Styled, UnderlineStyle, canvas, div, edge_faded, fill,
    linear_color_stop, linear_gradient, point, px, rgb, rounded_clip, size, white,
};

const TOLERANCE: f32 = 3.0 / 255.0;

fn channel(image: &RgbaImage, x: u32, y: u32, channel: usize) -> f32 {
    f32::from(image.get_pixel(x, y).0[channel]) / 255.0
}

fn assert_close(actual: f32, expected: f32, tolerance: f32, what: impl std::fmt::Debug) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{what:?}: rendered {actual}, expected {expected}"
    );
}

fn white_gradient() -> vespertine::Background {
    linear_gradient(
        180.0,
        linear_color_stop(white(), 0.0),
        linear_color_stop(white(), 1.0),
    )
}

fn white_image() -> Arc<RenderImage> {
    let frame = Frame::new(ImageBuffer::from_pixel(
        24,
        40,
        Rgba([255u8, 255, 255, 255]),
    ));
    Arc::new(RenderImage::new(vec![frame]))
}

/// Three probes in a flex row: a gradient quad (4..28), a color image (36..60), and a tall
/// glyph (68..96); all 40 logical pixels high from y = 4.
fn probes() -> vespertine::Div {
    let image = white_image();
    div()
        .flex()
        .size_full()
        .child(
            div()
                .flex_shrink_0()
                .m(px(4.0))
                .w(px(24.0))
                .h(px(40.0))
                .bg(white_gradient()),
        )
        .child(
            canvas(
                |_, _, _| (),
                move |area, _, window, _| {
                    window
                        .paint_image(area, area, Corners::default(), image, 0, false)
                        .expect("paint image");
                },
            )
            .flex_shrink_0()
            .m(px(4.0))
            .w(px(24.0))
            .h(px(40.0)),
        )
        .child(
            div()
                .flex_shrink_0()
                .m(px(4.0))
                .w(px(28.0))
                .h(px(40.0))
                .text_size(px(34.0))
                .line_height(px(34.0))
                .text_color(white())
                .child("W"),
        )
}

// ---- A4: E x K x opacity on a glyph, an image and a gradient quad ----

#[test]
fn edge_fade_half_opacity_and_rounded_clip_multiply_at_scale_factors_one_and_two() {
    let mut host = RenderHost::new();
    for scale in [1.0f32, 2.0] {
        let reference =
            host.render_at_scale(100.0, 48.0, scale, |_, _| probes().into_any_element());
        let scoped = host.render_at_scale(100.0, 48.0, scale, |_, _| {
            div()
                .size_full()
                .opacity(0.5)
                .child(
                    edge_faded(px(20.0), true, false, rounded_clip(px(10.0), probes()))
                        .fade_left(true),
                )
                .into_any_element()
        });
        assert_eq!(scoped.dimensions(), reference.dimensions());
        // The scope in device pixels, written out independently of the window code: top and
        // left fade bands of 20 logical pixels, rounded clip of the window with radius 10.
        let s = scale;
        let scope = ScopeParams {
            fade_edges: [0.0; 4],
            fade_bands: [20.0 * s, 0.0, 20.0 * s, 0.0],
            clip_bounds: [0.0, 0.0, 100.0 * s, 48.0 * s],
            clip_radius: 10.0 * s,
            ..Default::default()
        };
        let mut partial = [0usize; 3];
        for y in 0..(48.0 * s) as u32 {
            for x in 0..(100.0 * s) as u32 {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let f = scope.factor(p);
                let (lx, ly) = (p[0] / s, p[1] / s);
                let probe = if (4.0..28.0).contains(&lx) && (4.0..44.0).contains(&ly) {
                    0
                } else if (36.0..60.0).contains(&lx) && (4.0..44.0).contains(&ly) {
                    1
                } else if lx >= 64.0 {
                    2
                } else {
                    // Outside every probe the window stays black.
                    for c in 0..3 {
                        assert_eq!(scoped.get_pixel(x, y).0[c], 0, "({x}, {y})");
                    }
                    continue;
                };
                if probe == 1 {
                    // Bilinear sampling softens the last device pixels of the image tile
                    // against its atlas neighbors; that is texture filtering, not the scope.
                    let (dx, dy) = (p[0] - 36.0 * s, p[1] - 4.0 * s);
                    if dx < 2.0 || dy < 2.0 || dx > 24.0 * s - 2.0 || dy > 40.0 * s - 2.0 {
                        continue;
                    }
                }
                for c in 0..3 {
                    let actual = channel(&scoped, x, y, c);
                    let expected = if probe == 2 {
                        // The glyph: its unscoped coverage times opacity and the factor.
                        channel(&reference, x, y, c) * 0.5 * f
                    } else {
                        0.5 * f
                    };
                    assert_close(actual, expected, TOLERANCE * 1.5, (scale, probe, x, y, c));
                }
                if (0.1..0.9).contains(&f) && (probe != 2 || channel(&reference, x, y, 0) > 0.3) {
                    partial[probe] += 1;
                }
            }
        }
        for (probe, count) in partial.iter().enumerate() {
            assert!(
                *count > 0,
                "scale {scale}: probe {probe} never met the fade or the curve"
            );
        }
    }
}

// ---- A7: nested scopes and wrapper content on a transparent window ----

/// An opaque two-stop gradient background.
fn tint(top: u32, bottom: u32) -> vespertine::Background {
    linear_gradient(
        180.0,
        linear_color_stop(rgb(top), 0.0),
        linear_color_stop(rgb(bottom), 1.0),
    )
}

/// A canvas painting a gradient quad larger than its bounds: content that the scope has to
/// cut.
fn oversized(top: u32, bottom: u32) -> AnyElement {
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let all = Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(100.0), px(80.0)),
            };
            window.paint_quad(fill(all, tint(top, bottom)));
        },
    )
    .absolute()
    .size_full()
    .into_any_element()
}

#[test]
fn nested_scopes_at_scale_two_leave_a_transparent_window_untouched() {
    let mut host = RenderHost::new_transparent();
    // Outer wrapper (10,10) 80 x 60 radius 24; inner wrapper (12,12) 30 x 30 radius 6, whose
    // corner pokes out of the outer curve: its own geometry replaces the outer one.
    let image = host.render_logical(100.0, 80.0, |_, _| {
        div()
            .size_full()
            .child(
                div()
                    .absolute()
                    .left(px(10.0))
                    .top(px(10.0))
                    .w(px(80.0))
                    .h(px(60.0))
                    .child(rounded_clip(
                        px(24.0),
                        div()
                            .size_full()
                            .child(oversized(0x406090, 0x304870))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(2.0))
                                    .top(px(2.0))
                                    .w(px(30.0))
                                    .h(px(30.0))
                                    .child(rounded_clip(
                                        px(6.0),
                                        div().size_full().child(oversized(0xd08040, 0xe4a060)),
                                    )),
                            ),
                    )),
            )
            .into_any_element()
    });
    assert_eq!(image.dimensions(), (200, 160));
    let outer = ScopeParams {
        clip_bounds: [20.0, 20.0, 180.0, 140.0],
        clip_radius: 48.0,
        ..Default::default()
    };
    let inner = ScopeParams {
        clip_bounds: [24.0, 24.0, 84.0, 84.0],
        clip_radius: 12.0,
        ..Default::default()
    };
    let in_rect =
        |p: [f32; 2], b: [f32; 4]| p[0] >= b[0] && p[0] <= b[2] && p[1] >= b[1] && p[1] <= b[3];
    let mut poked_out = 0;
    for y in 0..160u32 {
        for x in 0..200u32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            // Opaque content cut per fragment: the premultiplied alpha is the coverage.
            let a_outer = if in_rect(p, outer.clip_bounds) {
                outer.rounded_coverage(p)
            } else {
                0.0
            };
            let a_inner = if in_rect(p, inner.clip_bounds) && in_rect(p, outer.clip_bounds) {
                inner.rounded_coverage(p)
            } else {
                0.0
            };
            let expected = a_outer + a_inner - a_outer * a_inner;
            assert_close(channel(&image, x, y, 3), expected, TOLERANCE * 1.5, (x, y));
            if expected == 0.0 {
                assert_eq!(
                    image.get_pixel(x, y).0,
                    [0, 0, 0, 0],
                    "({x}, {y}) must stay transparent"
                );
            }
            if a_outer == 0.0 && a_inner > 0.9 {
                poked_out += 1;
            }
        }
    }
    assert!(
        poked_out > 0,
        "the inner geometry must replace the outer curve"
    );
}

#[test]
fn wrapper_text_outside_the_curve_leaves_a_transparent_window_transparent() {
    let mut host = RenderHost::new_transparent();
    let image = host.render_logical(120.0, 60.0, |_, _| {
        div()
            .size_full()
            .child(rounded_clip(
                px(30.0),
                div()
                    .size_full()
                    .bg(tint(0x406090, 0x304870))
                    .text_color(white())
                    .text_size(px(22.0))
                    .child("Clipped text across the corner"),
            ))
            .into_any_element()
    });
    let scope = ScopeParams {
        clip_bounds: [0.0, 0.0, 240.0, 120.0],
        clip_radius: 60.0,
        ..Default::default()
    };
    let (mut cut, mut painted) = (0, 0);
    for y in 0..120u32 {
        for x in 0..240u32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let k = scope.rounded_coverage(p);
            if k == 0.0 {
                assert_eq!(image.get_pixel(x, y).0, [0, 0, 0, 0], "({x}, {y})");
                cut += 1;
            } else {
                // Text over the gradient overlaps at the antialiased edge, so the alpha is
                // at least the coverage of the fill (never less).
                let alpha = channel(&image, x, y, 3);
                assert!(
                    alpha >= k - TOLERANCE * 1.5,
                    "({x}, {y}): alpha {alpha} below coverage {k}"
                );
                painted += 1;
            }
        }
    }
    assert!(cut > 100 && painted > 10_000);
}

// ---- A6 / R7: path, underline and shadow keep only the rectangular mask ----

#[test]
fn explicit_scope_leaves_path_underline_and_shadow_rectangular_but_cuts_a_gradient() {
    // Content in the cut-away top-left corner of a scope with radius 40, painted under
    // radius 40 and under radius 0: for paths, underlines and shadows the two renders are
    // identical (R7), while a gradient-filled quad differs.
    let render = |radius: f32, content: fn(&mut vespertine::Window)| {
        RenderHost::new_transparent().render_logical(160.0, 120.0, move |_, _| {
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px(20.0))
                        .top(px(20.0))
                        .w(px(120.0))
                        .h(px(80.0))
                        .child(rounded_clip(
                            px(radius),
                            canvas(|_, _, _| (), move |_, _, window, _| content(window))
                                .size_full(),
                        )),
                )
                .into_any_element()
        })
    };
    let path: fn(&mut vespertine::Window) = |window| {
        let mut builder = PathBuilder::fill();
        builder.move_to(point(px(20.0), px(20.0)));
        builder.line_to(point(px(70.0), px(20.0)));
        builder.line_to(point(px(20.0), px(70.0)));
        builder.close();
        window.paint_path(builder.build().expect("path"), white());
    };
    let underline: fn(&mut vespertine::Window) = |window| {
        window.paint_underline(
            point(px(22.0), px(22.0)),
            px(60.0),
            &UnderlineStyle {
                thickness: px(3.0),
                color: Some(white()),
                wavy: false,
            },
        );
    };
    let shadow: fn(&mut vespertine::Window) = |window| {
        window.paint_drop_shadows(
            Bounds {
                origin: point(px(26.0), px(26.0)),
                size: size(px(24.0), px(24.0)),
            },
            Corners::all(px(4.0)),
            &[BoxShadow {
                color: white().opacity(0.9),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(4.0),
                spread_radius: px(0.0),
                inset: false,
            }],
        );
    };
    let gradient_quad: fn(&mut vespertine::Window) = |window| {
        window.paint_quad(fill(
            Bounds {
                origin: point(px(20.0), px(20.0)),
                size: size(px(50.0), px(50.0)),
            },
            white_gradient(),
        ));
    };
    for (name, content) in [("path", path), ("underline", underline), ("shadow", shadow)] {
        let rounded = render(40.0, content);
        let square = render(0.0, content);
        assert_eq!(
            rounded.as_raw(),
            square.as_raw(),
            "{name} must not be rounded"
        );
        // The content really sits in the cut-away corner of the radius-40 scope.
        let curve = ScopeParams {
            clip_bounds: [40.0, 40.0, 280.0, 200.0],
            clip_radius: 80.0,
            ..Default::default()
        };
        let in_corner = (40..120u32)
            .flat_map(|y| (40..120u32).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                curve.rounded_coverage([x as f32 + 0.5, y as f32 + 0.5]) == 0.0
                    && rounded.get_pixel(x, y).0[3] > 40
            })
            .count();
        assert!(
            in_corner > 20,
            "{name} painted {in_corner} pixels in the cut-away corner"
        );
    }
    let rounded = render(40.0, gradient_quad);
    let square = render(0.0, gradient_quad);
    assert_ne!(rounded.as_raw(), square.as_raw());
    assert!(
        rounded.get_pixel(46, 46).0[3] < 3,
        "the gradient quad is cut away"
    );
    assert!(square.get_pixel(46, 46).0[3] > 250);
}

// ---- A9 and R11: opt-in curved overflow clipping through real elements ----

/// The FID-RND-002-4 element tree: nested containers with unrelated origins, different
/// radii and asymmetric thick borders, and a red rectangle under both. Borders are
/// painted with an almost transparent color so that the red rectangle's coverage can be
/// read from the alpha channel; the clip geometry does not depend on the border alpha.
fn nested_containers(flag: bool) -> AnyElement {
    let outer = div()
        .absolute()
        .left(px(40.0))
        .top(px(40.0))
        .w(px(420.0))
        .h(px(280.0))
        .rounded(px(40.0))
        .border_t(px(4.0))
        .border_r(px(12.0))
        .border_b(px(20.0))
        .border_l(px(8.0))
        .border_color(vespertine::rgba(0x20202001))
        .overflow_hidden();
    let inner = div()
        .absolute()
        .left(px(72.0))
        .top(px(36.0))
        .w(px(400.0))
        .h(px(230.0))
        .rounded(px(12.0))
        .border(px(6.0))
        .border_color(vespertine::rgba(0x16387801))
        .overflow_hidden();
    let red = div()
        .absolute()
        .left(px(-26.0))
        .top(px(-26.0))
        .w(px(500.0))
        .h(px(260.0))
        .bg(rgb(0xe53a40));
    let (outer, inner) = if flag {
        (outer.curved_overflow_clip(), inner.curved_overflow_clip())
    } else {
        (outer, inner)
    };
    div()
        .size_full()
        .child(outer.child(inner.child(red)))
        .into_any_element()
}

#[test]
fn nested_curved_overflow_clips_follow_independent_shapes_and_flag_off_is_rectangular() {
    // FID-RND-002-4 at scale factor 2, shapes written out by hand (R12): the padding edges
    // of the outer block (border box 80,80 840 x 560) and the inner block.
    let outer = CurvedClipShape {
        bounds: [96.0, 88.0, 896.0, 600.0],
        radii_x: [64.0, 56.0, 56.0, 64.0],
        radii_y: [72.0, 72.0, 40.0, 40.0],
    };
    let inner = CurvedClipShape {
        bounds: [252.0, 172.0, 1028.0, 608.0],
        radii_x: [12.0; 4],
        radii_y: [12.0; 4],
    };
    let mut host = RenderHost::new_transparent();
    let on = host.render_logical(700.0, 420.0, |_, _| nested_containers(true));
    let off = host.render_logical(700.0, 420.0, |_, _| nested_containers(false));
    assert_eq!(on.dimensions(), (1400, 840));

    let (mut partial, mut cut_by_outer, mut cut_by_inner) = (0, 0, 0);
    for y in 0..840u32 {
        for x in 0..1400u32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let (a, b) = (outer.coverage(p), inner.coverage(p));
            let expected = a.min(b);
            assert_close(
                channel(&on, x, y, 3),
                expected,
                TOLERANCE * 1.5,
                ("alpha", x, y),
            );
            // Premultiplied red: the red channel carries the same coverage.
            assert_close(
                channel(&on, x, y, 0),
                expected * f32::from(0xe5u8) / 255.0,
                TOLERANCE * 1.5,
                ("red", x, y),
            );
            if expected > 0.05 && expected < 0.95 {
                partial += 1;
            }
            if b > 0.99 && a < 0.01 {
                cut_by_outer += 1;
            }
            if a > 0.99 && b < 0.01 && p[1] > 172.0 {
                cut_by_inner += 1;
            }
        }
    }
    assert!(partial > 40, "antialiased arcs of both shapes: {partial}");
    assert!(
        cut_by_outer > 1000,
        "the inner block overhangs the outer shape"
    );
    assert!(
        cut_by_inner > 1000,
        "the outer padding box exceeds the inner shape"
    );

    // Flag off: today's rectangular overflow masking, the intersection of the two padding
    // boxes (outer 48..448 x 44..300, inner 126..514 x 86..304 logical), exactly.
    for y in 0..840u32 {
        for x in 0..1400u32 {
            let inside = (252..896).contains(&x) && (172..600).contains(&y);
            let alpha = off.get_pixel(x, y).0[3];
            if inside {
                let pixel = off.get_pixel(x, y).0;
                for (actual, expected) in pixel.iter().zip([0xe5u8, 0x3a, 0x40, 0xff]) {
                    assert!(actual.abs_diff(expected) <= 2, "({x}, {y}) is {pixel:?}");
                }
            } else {
                assert!(alpha <= 3, "({x}, {y}) outside the masks: alpha {alpha}");
            }
        }
    }
}

#[test]
fn curved_overflow_flag_is_effective_only_with_hidden_overflow_and_a_positive_radius() {
    use vespertine::Style;
    let bounds = Bounds {
        origin: point(px(10.0), px(10.0)),
        size: size(px(100.0), px(60.0)),
    };
    let shape = |configure: &dyn Fn(&mut Style)| {
        let mut style = Style::default();
        style.curved_overflow_clip = true;
        style.overflow.x = vespertine::Overflow::Hidden;
        style.overflow.y = vespertine::Overflow::Hidden;
        let radius: vespertine::AbsoluteLength = px(8.0).into();
        style.corner_radii = vespertine::Corners {
            top_left: radius,
            top_right: radius,
            bottom_right: radius,
            bottom_left: radius,
        };
        configure(&mut style);
        style.curved_overflow_clip_shape(bounds, px(16.0), 2.0)
    };
    assert!(shape(&|_| {}).is_some());
    // Flag off.
    assert!(shape(&|style| style.curved_overflow_clip = false).is_none());
    // Overflow visible on either axis.
    assert!(shape(&|style| style.overflow.x = vespertine::Overflow::Visible).is_none());
    assert!(shape(&|style| style.overflow.y = vespertine::Overflow::Visible).is_none());
    // Scrolling counts as non-visible.
    assert!(
        shape(&|style| {
            style.overflow.x = vespertine::Overflow::Scroll;
            style.overflow.y = vespertine::Overflow::Scroll;
        })
        .is_some()
    );
    // No positive radius.
    assert!(shape(&|style| style.corner_radii = vespertine::Corners::default()).is_none());
    // One positive radius is enough; radii clamp to the quad like the background does.
    let single = shape(&|style| {
        style.corner_radii = vespertine::Corners::default();
        style.corner_radii.bottom_left = px(1000.0).into();
    })
    .expect("one positive radius opts in");
    // Clamped to half of the shorter side: 30 logical pixels = 60 device pixels.
    assert_eq!(single.radii_x[3], 60.0);
    assert_eq!(single.radii_y[3], 60.0);
}

// ---- R15: C multiplies fills, glyphs and strikethroughs of an opted-in element ----

#[test]
fn curved_clip_multiplies_fills_glyphs_and_strikethroughs_of_opted_in_elements() {
    // Three side by side rounded containers whose single, non-overlapping content is a
    // plain solid fill, a glyph run and a strikethrough. With the flag on, every pixel
    // equals the flag-off pixel times the container's curved coverage (the target is an
    // opaque black window, so the blend is linear in the coverage).
    let tree = |flag: bool| {
        move |_: &mut vespertine::Window, _: &mut vespertine::App| {
            let container = |index: usize| {
                let element = div()
                    .absolute()
                    .left(px(10.0 + 110.0 * index as f32))
                    .top(px(10.0))
                    .w(px(100.0))
                    .h(px(40.0))
                    .rounded(px(14.0))
                    .overflow_hidden();
                if flag {
                    element.curved_overflow_clip()
                } else {
                    element
                }
            };
            div()
                .size_full()
                .child(container(0).child(div().size_full().bg(rgb(0x2c6de8))))
                .child(
                    container(1).child(div().text_color(white()).text_size(px(26.0)).child("Hig")),
                )
                .child(
                    container(2).child(
                        canvas(
                            |_, _, _| (),
                            |_, _, window, _| {
                                window.paint_strikethrough(
                                    point(px(226.0), px(14.0)),
                                    px(90.0),
                                    &vespertine::StrikethroughStyle {
                                        thickness: px(3.0),
                                        color: Some(white()),
                                    },
                                );
                                window.paint_strikethrough(
                                    point(px(226.0), px(44.0)),
                                    px(90.0),
                                    &vespertine::StrikethroughStyle {
                                        thickness: px(3.0),
                                        color: Some(rgb(0xe0a040).into()),
                                    },
                                );
                            },
                        )
                        .size_full(),
                    ),
                )
                .into_any_element()
        }
    };
    let mut host = RenderHost::new();
    let off = host.render_logical(340.0, 60.0, tree(false));
    let on = host.render_logical(340.0, 60.0, tree(true));
    let shapes: Vec<CurvedClipShape> = (0..3)
        .map(|index| {
            let left = (10.0 + 110.0 * index as f32) * 2.0;
            CurvedClipShape {
                bounds: [left, 20.0, left + 200.0, 100.0],
                radii_x: [28.0; 4],
                radii_y: [28.0; 4],
            }
        })
        .collect();
    let mut partial = [0usize; 3];
    for y in 0..120u32 {
        for x in 0..680u32 {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let index = ((x as usize) / 220).min(2);
            let c = shapes[index].coverage(p);
            for ch in 0..3 {
                let base = channel(&off, x, y, ch);
                assert_close(
                    channel(&on, x, y, ch),
                    base * c,
                    TOLERANCE * 1.5,
                    (index, x, y, ch),
                );
            }
            if c > 0.05 && c < 0.95 && channel(&off, x, y, 0) + channel(&off, x, y, 2) > 0.2 {
                partial[index] += 1;
            }
        }
    }
    // The fill and the strikethrough really met the curved edge; the glyph run must at
    // least have painted inside its container.
    assert!(partial[0] > 0 && partial[2] > 0, "{partial:?}");
    let glyph_pixels = (440..660u32)
        .flat_map(|x| (20..100u32).map(move |y| (x, y)))
        .filter(|&(x, y)| off.get_pixel(x, y).0[0] > 128)
        .count();
    assert!(glyph_pixels > 50, "glyph run painted {glyph_pixels} pixels");
}
