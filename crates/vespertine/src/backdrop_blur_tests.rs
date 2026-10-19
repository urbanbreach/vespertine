//! Tests of the public backdrop-blur paint operation and its scene records (RND-003).

use crate::{
    BackdropBlur, Bounds, ContentMask, Corners, Hsla, Pixels, Scene, Shadow, Styled,
    TestAppContext, Window, canvas, fill, point, px, size,
};

fn bounds(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}

struct Painted {
    blurs: Vec<BackdropBlur>,
    shadows: Vec<Shadow>,
    quad_orders: Vec<u32>,
    scene: Scene,
}

/// Paints with `paint` at the given scale factor and returns the scene records of the frame.
fn paint(
    cx: &mut TestAppContext,
    scale_factor: f32,
    paint: impl Fn(&mut Window) + 'static,
) -> Painted {
    let cx = cx.add_empty_window();
    cx.simulate_scale_factor_change(scale_factor);
    cx.draw(point(px(0.), px(0.)), size(px(300.), px(200.)), |_, _| {
        canvas(|_, _, _| (), move |_, _, window, _| paint(window)).size_full()
    });
    cx.update(|window, _| {
        let scene = &window.rendered_frame.scene;
        let mut copy = Scene::default();
        copy.replay(0..scene.len(), scene);
        copy.finish();
        Painted {
            blurs: scene.backdrop_blurs.clone(),
            shadows: scene.shadows.clone(),
            quad_orders: scene.quads.iter().map(|quad| quad.order).collect(),
            scene: copy,
        }
    })
}

fn radii() -> Corners<Pixels> {
    Corners {
        top_left: px(1.),
        top_right: px(2.),
        bottom_right: px(3.),
        bottom_left: px(4.),
    }
}

#[vespertine::test]
fn effect_and_ordering_primitive_are_scaled_by_the_scale_factor(cx: &mut TestAppContext) {
    for scale in [1.0_f32, 2.0] {
        let painted = paint(cx, scale, |window| {
            window.paint_backdrop_blur(bounds(10., 20., 100., 50.), radii(), px(8.));
        });
        assert_eq!(painted.blurs.len(), 1, "scale {scale}");
        let blur = painted.blurs[0];
        assert_eq!(
            blur.blur_radius.0,
            8.0 * scale,
            "device sigma equals radius times scale"
        );
        assert_eq!(blur.bounds.origin.x.0, 10.0 * scale);
        assert_eq!(blur.bounds.origin.y.0, 20.0 * scale);
        assert_eq!(blur.bounds.size.width.0, 100.0 * scale);
        assert_eq!(blur.bounds.size.height.0, 50.0 * scale);
        assert_eq!(blur.corner_radii.top_left.0, 1.0 * scale);
        assert_eq!(blur.corner_radii.top_right.0, 2.0 * scale);
        assert_eq!(blur.corner_radii.bottom_right.0, 3.0 * scale);
        assert_eq!(blur.corner_radii.bottom_left.0, 4.0 * scale);

        // An invisible zero-blur, transparent shadow with the same geometry precedes it.
        assert_eq!(painted.shadows.len(), 1);
        let shadow = painted.shadows[0];
        assert_eq!(shadow.blur_radius.0, 0.0);
        assert_eq!(shadow.color, Hsla::transparent_black());
        assert_eq!(shadow.bounds, blur.bounds);
        assert_eq!(shadow.corner_radii, blur.corner_radii);
        assert_eq!(shadow.content_mask, blur.content_mask);
        assert_eq!(
            shadow.order, blur.order,
            "the ordering primitive shares the effect order"
        );
    }
}

#[vespertine::test]
fn zero_and_negative_radius_emit_nothing(cx: &mut TestAppContext) {
    for radius in [0.0, -3.0, f32::NAN] {
        let painted = paint(cx, 2.0, move |window| {
            window.paint_backdrop_blur(bounds(10., 20., 100., 50.), radii(), px(radius));
        });
        assert!(painted.blurs.is_empty(), "radius {radius}");
        assert!(painted.shadows.is_empty(), "radius {radius}");
    }
}

#[vespertine::test]
fn the_content_mask_clips_the_effect_and_drops_fully_clipped_ones(cx: &mut TestAppContext) {
    let painted = paint(cx, 2.0, |window| {
        window.with_content_mask(
            Some(ContentMask {
                bounds: bounds(20., 30., 40., 20.),
            }),
            |window| {
                window.paint_backdrop_blur(bounds(10., 20., 100., 50.), radii(), px(4.));
            },
        );
    });
    assert_eq!(painted.blurs.len(), 1);
    let mask = painted.blurs[0].content_mask.bounds;
    assert_eq!(mask.origin.x.0, 40.0);
    assert_eq!(mask.origin.y.0, 60.0);
    assert_eq!(mask.size.width.0, 80.0);
    assert_eq!(mask.size.height.0, 40.0);

    let painted = paint(cx, 1.0, |window| {
        window.with_content_mask(
            Some(ContentMask {
                bounds: bounds(200., 150., 20., 20.),
            }),
            |window| {
                window.paint_backdrop_blur(bounds(10., 20., 100., 50.), radii(), px(4.));
            },
        );
    });
    assert!(
        painted.blurs.is_empty(),
        "an effect with empty clipped bounds is dropped"
    );
    assert!(painted.shadows.is_empty());
}

#[vespertine::test]
fn a_layer_shares_one_order_and_nested_layers_draw_above_it(cx: &mut TestAppContext) {
    let painted = paint(cx, 1.0, |window| {
        // An unrelated primitive underneath, overlapping the card.
        window.paint_quad(fill(bounds(0., 0., 300., 200.), crate::red()));
        let card = bounds(40., 30., 160., 100.);
        window.paint_layer(card, |window| {
            window.paint_backdrop_blur(card, radii(), px(8.));
            window.paint_quad(fill(card, crate::blue()));
            window.paint_layer(bounds(60., 50., 40., 20.), |window| {
                window.paint_quad(fill(bounds(60., 50., 40., 20.), crate::green()));
            });
        });
    });
    assert_eq!(painted.blurs.len(), 1);
    let card_order = painted.blurs[0].order;
    assert_eq!(
        painted.shadows[0].order, card_order,
        "the ordering primitive shares the layer order"
    );
    // quad orders in paint order: background, tint, overlay.
    let mut by_paint = painted.quad_orders;
    by_paint.sort_unstable();
    assert!(by_paint[0] < card_order);
    assert_eq!(by_paint[1], card_order, "the tint shares the card's order");
    assert!(
        by_paint[2] > card_order,
        "the nested overlay draws above the card"
    );
}

#[vespertine::test]
fn sibling_layers_get_increasing_orders(cx: &mut TestAppContext) {
    let painted = paint(cx, 1.0, |window| {
        for index in 0..3 {
            let panel = bounds(20. + 30. * index as f32, 20., 120., 100.);
            window.paint_layer(panel, |window| {
                window.paint_backdrop_blur(panel, Corners::default(), px(6.));
                window.paint_quad(fill(panel, crate::blue()));
            });
        }
    });
    assert_eq!(painted.blurs.len(), 3);
    assert!(painted.blurs[0].order < painted.blurs[1].order);
    assert!(painted.blurs[1].order < painted.blurs[2].order);
}

#[vespertine::test]
fn replaying_a_retained_range_reproduces_the_effect(cx: &mut TestAppContext) {
    let painted = paint(cx, 2.0, |window| {
        let card = bounds(40., 30., 160., 100.);
        window.paint_layer(card, |window| {
            window.paint_backdrop_blur(card, radii(), px(8.));
            window.paint_quad(fill(card, crate::blue()));
        });
    });
    // `Painted::scene` was built by replaying the whole frame into a fresh scene.
    assert_eq!(painted.scene.backdrop_blurs.len(), 1);
    assert_eq!(
        painted.scene.backdrop_blurs[0].bounds,
        painted.blurs[0].bounds
    );
    assert_eq!(
        painted.scene.backdrop_blurs[0].blur_radius,
        painted.blurs[0].blur_radius
    );

    // Replaying into a scene that already holds an overlapping primitive re-derives the order
    // like an ordinary primitive, keeping the effect's order equal to its layer's contents.
    let mut next = Scene::default();
    next.insert_primitive(crate::Quad {
        order: 0,
        bounds: painted.blurs[0].bounds,
        content_mask: painted.blurs[0].content_mask,
        ..Default::default()
    });
    next.replay(0..painted.scene.len(), &painted.scene);
    next.finish();
    assert_eq!(next.backdrop_blurs.len(), 1);
    let tint_order = next.quads.last().map(|quad| quad.order).unwrap();
    assert_eq!(next.backdrop_blurs[0].order, tint_order);
    assert!(tint_order > next.quads[0].order);
}

#[vespertine::test]
fn batches_are_cut_at_effect_orders(cx: &mut TestAppContext) {
    let painted = paint(cx, 1.0, |window| {
        // Quads below and above the effect would coalesce into one batch without the cut.
        window.paint_quad(fill(bounds(0., 0., 300., 200.), crate::red()));
        window.paint_backdrop_blur(bounds(50., 50., 100., 100.), Corners::default(), px(5.));
        window.paint_quad(fill(bounds(50., 50., 100., 100.), crate::blue()));
    });
    let batches: Vec<_> = painted.scene.batches().collect();
    let blur_order = painted.scene.backdrop_blurs[0].order;
    let mut seen_below_only = false;
    for batch in &batches {
        let first = painted.scene.batch_first_order(batch);
        if first < blur_order {
            seen_below_only = true;
            if let crate::PrimitiveBatch::Quads(range) = batch {
                for quad in &painted.scene.quads[range.clone()] {
                    assert!(
                        quad.order < blur_order,
                        "a batch must not span the effect's order"
                    );
                }
            }
        }
    }
    assert!(seen_below_only);
}
