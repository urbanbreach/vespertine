//! RND-008 / RND-009 scene-level acceptance tests: the canonical layer order of a surface
//! plate, per-corner radii, nested opacity and the shared quad contract.

use crate::{
    BoxShadow, Corners, Hsla, IntoElement, PaintOperation, ParentElement, Primitive, Quad,
    ScaledPixels, Scene, Shadow, Styled, TestAppContext, black, div, linear_color_stop,
    linear_gradient, point, px, rgb, size, white,
};

#[derive(Debug, PartialEq, Eq)]
enum Layer {
    DropShadow,
    Fill,
    InsetShadow,
    Child,
    Border,
}

fn drop_shadow() -> BoxShadow {
    BoxShadow {
        color: Hsla { a: 0.4, ..black() },
        offset: point(px(0.), px(6.)),
        blur_radius: px(12.),
        spread_radius: px(0.),
        inset: false,
    }
}

fn inset_shadow() -> BoxShadow {
    BoxShadow {
        color: Hsla { a: 0.2, ..white() },
        offset: point(px(0.), px(1.)),
        blur_radius: px(0.),
        spread_radius: px(0.),
        inset: true,
    }
}

fn plate() -> impl IntoElement {
    div()
        .w(px(240.))
        .h(px(100.))
        .rounded(px(12.))
        .bg(linear_gradient(
            180.,
            linear_color_stop(rgb(0x505866), 0.),
            linear_color_stop(rgb(0x303640), 1.),
        ))
        .border_1()
        .border_color(rgb(0x8993A3))
        .shadow(vec![drop_shadow(), inset_shadow()])
        .child(div().size(px(20.)).bg(rgb(0xff0000)))
}

fn classify(primitive: &Primitive) -> Option<Layer> {
    match primitive {
        Primitive::Shadow(shadow) if shadow.inset == 0 => Some(Layer::DropShadow),
        Primitive::Shadow(_) => Some(Layer::InsetShadow),
        Primitive::Quad(quad) => {
            let widths = quad.border_widths;
            if widths.top.0 != 0.
                || widths.right.0 != 0.
                || widths.bottom.0 != 0.
                || widths.left.0 != 0.
            {
                Some(Layer::Border)
            } else if quad.background.tag == crate::BackgroundTag::LinearGradient {
                Some(Layer::Fill)
            } else {
                Some(Layer::Child)
            }
        }
        _ => None,
    }
}

fn painted(cx: &mut TestAppContext, element: impl Fn() -> crate::AnyElement + 'static) -> Scene {
    let window = cx.add_empty_window();
    window.draw(
        point(px(0.), px(0.)),
        size(px(400.), px(300.)),
        move |_, _| element(),
    );
    window.update(|window, _| {
        let mut scene = Scene::default();
        let source = &window.rendered_frame.scene;
        scene.replay(0..source.len(), source);
        scene
    })
}

fn quads(scene: &Scene) -> Vec<Quad> {
    scene.quads.clone()
}

fn shadows(scene: &Scene) -> Vec<Shadow> {
    scene.shadows.clone()
}

#[vespertine::test]
fn styled_plate_paints_layers_in_canonical_order(cx: &mut TestAppContext) {
    let scene = painted(cx, || plate().into_any_element());
    let mut layers = Vec::new();
    for operation in &scene.paint_operations {
        if let PaintOperation::Primitive(primitive) = operation
            && let Some(layer) = classify(primitive)
        {
            layers.push(layer);
        }
    }
    // Drop shadow, fill, inset shadow, children, border strips last.
    assert_eq!(
        &layers[..4],
        &[
            Layer::DropShadow,
            Layer::Fill,
            Layer::InsetShadow,
            Layer::Child
        ]
    );
    assert!(layers.len() > 4);
    assert!(layers[4..].iter().all(|layer| *layer == Layer::Border));
}

#[vespertine::test]
fn nested_opacity_multiplies_without_changing_layout(cx: &mut TestAppContext) {
    let nested = painted(cx, || {
        div()
            .opacity(0.5)
            .child(div().opacity(0.5).size(px(40.)).bg(white()))
            .into_any_element()
    });
    let plain = painted(cx, || {
        div()
            .child(div().size(px(40.)).bg(white()))
            .into_any_element()
    });
    let nested_quad = quads(&nested).pop().expect("nested quad");
    let plain_quad = quads(&plain).pop().expect("plain quad");
    assert!((nested_quad.background.solid.a - 0.25).abs() < 1e-6);
    assert_eq!(nested_quad.bounds, plain_quad.bounds);
}

#[vespertine::test]
fn per_corner_radii_are_shared_by_fill_border_and_shadows(cx: &mut TestAppContext) {
    let scene = painted(cx, || {
        div()
            .w(px(100.))
            .h(px(60.))
            .rounded_tl(px(12.))
            .rounded_br(px(12.))
            .bg(rgb(0x404860))
            .border_1()
            .border_color(rgb(0x8993A3))
            .shadow(vec![drop_shadow(), inset_shadow()])
            .into_any_element()
    });
    let expected = Corners {
        top_left: ScaledPixels(24.),
        top_right: ScaledPixels(0.),
        bottom_right: ScaledPixels(24.),
        bottom_left: ScaledPixels(0.),
    };
    let quads = quads(&scene);
    assert!(quads.len() >= 2);
    assert!(quads.iter().all(|quad| quad.corner_radii == expected));
    let shadows = shadows(&scene);
    assert_eq!(shadows.len(), 2);
    for shadow in shadows {
        assert_eq!(shadow.corner_radii, expected);
        assert_eq!(shadow.element_corner_radii, expected);
    }
}

/// RND-009 A4: a hitbox inserted in the canvas prepaint receives pointer hits, the unmarked
/// canvas area does not, and an ancestor's clip excludes hits outside the clip.
#[vespertine::test]
fn canvas_hitboxes_come_only_from_prepaint_and_respect_ancestor_clips(cx: &mut TestAppContext) {
    use crate::{Bounds, HitboxBehavior, canvas, point};
    use std::{cell::RefCell, rc::Rc};

    let hitboxes = Rc::new(RefCell::new(Vec::new()));
    let window = cx.add_empty_window();
    let recorded = hitboxes.clone();
    window.draw(
        point(px(0.), px(0.)),
        size(px(300.), px(200.)),
        move |_, _| {
            div()
                .absolute()
                .left(px(20.))
                .top(px(20.))
                .w(px(100.))
                .h(px(100.))
                .overflow_hidden()
                .child(
                    canvas(
                        move |bounds: Bounds<crate::Pixels>, window, _| {
                            let inner = Bounds {
                                origin: bounds.origin + point(px(10.), px(10.)),
                                size: size(px(30.), px(30.)),
                            };
                            let beyond = Bounds {
                                origin: bounds.origin + point(px(80.), px(80.)),
                                size: size(px(100.), px(100.)),
                            };
                            recorded.borrow_mut().push((
                                window.insert_hitbox(inner, HitboxBehavior::Normal),
                                window.insert_hitbox(beyond, HitboxBehavior::Normal),
                            ));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .w(px(200.))
                    .h(px(200.)),
                )
                .into_any_element()
        },
    );
    let (inner, beyond) = hitboxes.borrow()[0].clone();
    window.update(|window, _| {
        // The root element is laid out at the draw origin: the clipping parent covers
        // (0, 0)-(100, 100), the first hitbox (10, 10)-(40, 40), the second (80, 80)-(180, 180).
        let hit = |hitbox: &crate::Hitbox, x: f32, y: f32| {
            window
                .rendered_frame
                .hit_test(point(px(x), px(y)))
                .ids
                .contains(&hitbox.id)
        };
        // Inside the inserted hitbox.
        assert!(hit(&inner, 20., 20.));
        // Unmarked canvas area (inside the clip, outside both hitboxes).
        assert!(!hit(&inner, 60., 20.));
        assert!(!hit(&beyond, 60., 20.));
        // Inside the second hitbox and inside the ancestor clip: hit.
        assert!(hit(&beyond, 90., 90.));
        // Inside the second hitbox but outside the ancestor clip: no hit.
        assert!(!hit(&beyond, 150., 150.));
    });
}
