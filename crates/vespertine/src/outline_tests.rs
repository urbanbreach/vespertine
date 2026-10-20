//! Tests for ELM-008, layout-neutral element outlines.

use crate::{
    AnyElement, AnyWindowHandle, AppContext as _, Bounds, Context, Hsla, InteractiveElement as _,
    IntoElement, Outline, ParentElement as _, Pixels, Quad, Render, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, TestAppContext, Window, div, point, px, rgb,
    size, transparent_black,
};
use std::rc::Rc;

struct Probe(Rc<dyn Fn() -> AnyElement>);

impl Render for Probe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        (self.0)()
    }
}

struct Painted {
    quads: Vec<Quad>,
    scale: f32,
    debug_bounds: Vec<(&'static str, Bounds<Pixels>)>,
    hitboxes: Vec<Bounds<Pixels>>,
}

fn paint(
    cx: &mut TestAppContext,
    selectors: &[&'static str],
    build: impl Fn() -> AnyElement + 'static,
) -> Painted {
    let build: Rc<dyn Fn() -> AnyElement> = Rc::new(build);
    let window = cx.add_window(move |_, _| Probe(build));
    let window = AnyWindowHandle::from(window);
    cx.update_window(window, |_, window, cx| {
        window.draw(cx).clear(cx);
        Painted {
            quads: window.painted_quads(),
            scale: window.scale_factor(),
            debug_bounds: selectors
                .iter()
                .filter_map(|selector| {
                    window
                        .rendered_frame
                        .debug_bounds
                        .get(*selector)
                        .map(|bounds| (*selector, *bounds))
                })
                .collect(),
            hitboxes: window
                .rendered_frame
                .hitboxes
                .iter()
                .map(|hitbox| hitbox.bounds)
                .collect(),
        }
    })
    .unwrap()
}

fn blue() -> Hsla {
    rgb(0x0066FF).into()
}

fn orange() -> Hsla {
    rgb(0xFF8000).into()
}

fn ring_quads(quads: &[Quad], color: Hsla) -> Vec<Quad> {
    quads
        .iter()
        .filter(|quad| quad.border_color == color && quad.background.is_transparent())
        .cloned()
        .collect()
}

fn scaled(bounds: Bounds<Pixels>, scale: f32) -> [f32; 4] {
    [
        bounds.origin.x.0 * scale,
        bounds.origin.y.0 * scale,
        bounds.size.width.0 * scale,
        bounds.size.height.0 * scale,
    ]
}

fn quad_bounds(quad: &Quad) -> [f32; 4] {
    [
        quad.bounds.origin.x.0,
        quad.bounds.origin.y.0,
        quad.bounds.size.width.0,
        quad.bounds.size.height.0,
    ]
}

fn assert_close(actual: [f32; 4], expected: [f32; 4]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 0.01, "{actual:?} != {expected:?}");
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(w), px(h)),
    }
}

#[test]
fn outline_radius_rule() {
    let outline = |w: f32, o: f32| Outline::new(px(w), blue(), px(o));
    assert_eq!(outline(2., 3.).outer_radius(px(8.)), px(13.));
    assert_eq!(outline(3., -5.).outer_radius(px(8.)), px(6.));
    assert_eq!(outline(2., 3.).outer_radius(px(0.)), px(0.));
    assert_eq!(outline(3., -5.).outer_radius(px(0.)), px(0.));
    assert_eq!(outline(1., -5.).outer_radius(px(2.)), px(0.));
}

#[test]
fn outline_outer_bounds_expand_by_offset_plus_width() {
    let border_box = rect(40., 50., 120., 80.);
    let outline = Outline::new(px(2.), blue(), px(3.));
    assert_eq!(
        outline.outer_bounds(border_box),
        Some(rect(35., 45., 130., 90.))
    );
    let inside = Outline::new(px(3.), blue(), px(-5.));
    assert_eq!(
        inside.outer_bounds(border_box),
        Some(rect(42., 52., 116., 76.))
    );
    // o + w = -60 makes the 80 high box inverted.
    let inverted = Outline::new(px(5.), blue(), px(-65.));
    assert_eq!(inverted.outer_bounds(border_box), None);
    assert_eq!(
        Outline::new(px(5.), blue(), px(-45.)).outer_bounds(rect(0., 0., 80., 80.)),
        None
    );
}

fn fid_scene() -> AnyElement {
    div()
        .size_full()
        .relative()
        .bg(rgb(0xFFFFFF))
        .child(
            div()
                .absolute()
                .left(px(40.))
                .top(px(50.))
                .w(px(120.))
                .h(px(80.))
                .bg(rgb(0x202020))
                .rounded(px(8.))
                .outline(Outline::new(px(2.), blue(), px(3.))),
        )
        .child(
            div()
                .absolute()
                .left(px(220.))
                .top(px(50.))
                .w(px(120.))
                .h(px(80.))
                .bg(rgb(0x202020))
                .rounded_tr(px(8.))
                .rounded_br(px(8.))
                .rounded_bl(px(8.))
                .outline(Outline::new(px(3.), orange(), px(-5.))),
        )
        .into_any_element()
}

/// A4 (headless scene form): FID-ELM-008 geometry, including the square top-left corner.
#[vespertine::test]
fn fid_elm_008_outline_geometry(cx: &mut TestAppContext) {
    let painted = paint(cx, &[], fid_scene);
    let s = painted.scale;

    let first = ring_quads(&painted.quads, blue());
    assert!(!first.is_empty());
    for quad in &first {
        assert_close(quad_bounds(quad), scaled(rect(35., 45., 130., 90.), s));
        assert_close(
            [
                quad.corner_radii.top_left.0,
                quad.corner_radii.top_right.0,
                quad.corner_radii.bottom_right.0,
                quad.corner_radii.bottom_left.0,
            ],
            [13. * s; 4],
        );
        assert_close(
            [
                quad.border_widths.top.0,
                quad.border_widths.right.0,
                quad.border_widths.bottom.0,
                quad.border_widths.left.0,
            ],
            [2. * s; 4],
        );
    }

    let second = ring_quads(&painted.quads, orange());
    assert!(!second.is_empty());
    for quad in &second {
        assert_close(quad_bounds(quad), scaled(rect(222., 52., 116., 76.), s));
        assert_close(
            [
                quad.corner_radii.top_left.0,
                quad.corner_radii.top_right.0,
                quad.corner_radii.bottom_right.0,
                quad.corner_radii.bottom_left.0,
            ],
            [0., 6. * s, 6. * s, 6. * s],
        );
        assert_close(
            [
                quad.border_widths.top.0,
                quad.border_widths.right.0,
                quad.border_widths.bottom.0,
                quad.border_widths.left.0,
            ],
            [3. * s; 4],
        );
    }
}

/// A3: nothing is painted for zero width, negative width and a fully transparent color.
#[vespertine::test]
fn outline_paints_nothing_for_invisible_values(cx: &mut TestAppContext) {
    let build = |outline: Option<Outline>| {
        move || {
            let mut element = div().w(px(50.)).h(px(40.)).bg(rgb(0x202020));
            if let Some(outline) = outline {
                element = element.outline(outline);
            }
            div()
                .size_full()
                .p(px(20.))
                .child(element)
                .into_any_element()
        }
    };
    let baseline = paint(cx, &[], build(None)).quads.len();
    for outline in [
        Outline::new(px(0.), blue(), px(3.)),
        Outline::new(px(-2.), blue(), px(3.)),
        Outline::new(px(2.), transparent_black(), px(3.)),
        Outline::new(px(2.), blue().opacity(0.), px(3.)),
    ] {
        let painted = paint(cx, &[], build(Some(outline)));
        assert_eq!(painted.quads.len(), baseline, "{outline:?}");
    }
    let visible = paint(cx, &[], build(Some(Outline::new(px(2.), blue(), px(3.)))));
    assert!(visible.quads.len() > baseline);
}

/// A3: the outline is painted after the border and the children, and its alpha is
/// multiplied by the element opacity.
#[vespertine::test]
fn outline_paints_after_border_and_children(cx: &mut TestAppContext) {
    let painted = paint(cx, &[], || {
        div()
            .size_full()
            .p(px(20.))
            .child(
                div()
                    .w(px(100.))
                    .h(px(60.))
                    .opacity(0.5)
                    .bg(rgb(0x112233))
                    .border_2()
                    .border_color(rgb(0x00FF00))
                    // The ring lies inside the box and overlaps the border and the child, so
                    // paint order is observable in the primitives' draw order.
                    .outline(Outline::new(px(4.), blue(), px(-5.)))
                    .child(div().size(px(30.)).bg(rgb(0x445566))),
            )
            .into_any_element()
    });
    let same_hue = |a: Hsla, b: Hsla| a.h == b.h && a.s == b.s && a.l == b.l;
    let background = painted
        .quads
        .iter()
        .find(|q| q.bounds.size.width.0 == 100. * painted.scale && !q.background.is_transparent())
        .expect("background painted");
    let child = painted
        .quads
        .iter()
        .find(|q| q.bounds.size.width.0 == 30. * painted.scale)
        .expect("child painted");
    let border = painted
        .quads
        .iter()
        .find(|q| same_hue(q.border_color, Hsla::from(rgb(0x00FF00))))
        .expect("border painted");
    let rings: Vec<&Quad> = painted
        .quads
        .iter()
        .filter(|q| same_hue(q.border_color, blue()) && q.background.is_transparent())
        .collect();
    assert!(!rings.is_empty());
    for ring in rings {
        assert!(ring.order > background.order, "outline above background");
        assert!(ring.order > child.order, "outline above children");
        assert!(ring.order > border.order, "outline above border");
        assert!(
            (ring.border_color.a - 0.5).abs() < 1e-4,
            "opacity multiplies the outline alpha"
        );
    }
}

fn layout_tree(outline: Option<Outline>, handle: ScrollHandle) -> AnyElement {
    let mut scroller = div()
        .id("scroller")
        .debug_selector(|| "scroller".into())
        .w(px(100.))
        .h(px(100.))
        .overflow_scroll()
        .track_scroll(&handle)
        .cursor_pointer();
    if let Some(outline) = outline {
        scroller = scroller.outline(outline);
    }
    div()
        .size_full()
        .p(px(10.))
        .child(
            scroller.child(
                div()
                    .debug_selector(|| "content".into())
                    .w(px(300.))
                    .h(px(250.))
                    .child(div().debug_selector(|| "inner".into()).size(px(20.))),
            ),
        )
        .child(div().debug_selector(|| "sibling".into()).size(px(10.)))
        .into_any_element()
}

/// A2: geometry is identical with the outline enabled and disabled.
#[vespertine::test]
fn outline_is_layout_neutral(cx: &mut TestAppContext) {
    let selectors = ["scroller", "content", "inner", "sibling"];
    let run = |cx: &mut TestAppContext, outline: Option<Outline>| {
        let handle = ScrollHandle::new();
        let tree_handle = handle.clone();
        let painted = paint(cx, &selectors, move || {
            layout_tree(outline, tree_handle.clone())
        });
        (painted, handle.max_offset())
    };
    let (without, without_scroll) = run(cx, None);
    for outline in [
        Outline::new(px(4.), blue(), px(6.)),
        Outline::new(px(2.), blue(), px(-8.)),
        Outline::new(px(0.), blue(), px(6.)),
    ] {
        let (with, with_scroll) = run(cx, Some(outline));
        assert_eq!(with.debug_bounds, without.debug_bounds);
        assert_eq!(with.hitboxes, without.hitboxes);
        assert_eq!(with_scroll, without_scroll);
    }
    assert_eq!(without_scroll.y, px(150.));
    assert_eq!(without.debug_bounds.len(), selectors.len());
}

/// A6: an ancestor's content mask clips the outline; an element's own overflow clip does not.
#[vespertine::test]
fn outline_is_clipped_by_ancestor_but_not_by_own_overflow(cx: &mut TestAppContext) {
    let ancestor = rect(20., 20., 60., 60.);
    let painted = paint(cx, &[], move || {
        let clipper = div()
            .absolute()
            .left(px(20.))
            .top(px(20.))
            .w(px(60.))
            .h(px(60.))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .size(px(20.))
                    .outline(Outline::new(px(12.), blue(), px(13.))),
            );
        div()
            .relative()
            .size_full()
            .child(clipper)
            .child(
                div()
                    .absolute()
                    .left(px(200.))
                    .top(px(20.))
                    .size(px(40.))
                    .overflow_hidden()
                    .outline(Outline::new(px(4.), orange(), px(6.))),
            )
            .into_any_element()
    });
    let s = painted.scale;

    let clipped = ring_quads(&painted.quads, blue());
    assert!(!clipped.is_empty());
    let mask = scaled(ancestor, s);
    for quad in &clipped {
        let m = &quad.content_mask.bounds;
        assert!(m.origin.x.0 >= mask[0] - 0.01 && m.origin.y.0 >= mask[1] - 0.01);
        assert!(m.right().0 <= mask[0] + mask[2] + 0.01);
        assert!(m.bottom().0 <= mask[1] + mask[3] + 0.01);
    }

    // The outline of the element with its own hidden overflow sits entirely outside the
    // element's bounds and must still be painted.
    let unclipped = ring_quads(&painted.quads, orange());
    assert!(!unclipped.is_empty());
    let element = scaled(rect(200., 20., 40., 40.), s);
    for quad in &unclipped {
        assert_close(quad_bounds(quad), scaled(rect(190., 10., 60., 60.), s));
    }
    // All four sides of the ring are painted, including those left of the element's own clip.
    assert_eq!(unclipped.len(), 4);
    assert!(
        unclipped
            .iter()
            .any(|quad| quad.content_mask.bounds.origin.x.0 < element[0])
    );
}
