//! CMP-020 A1 to A6 (viewer geometry, zoom, resize, wheel, pinch, drag and click).

mod media_support;

use media_support::solid_image;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use vespertine::{
    Context, InteractiveElement, IntoElement, Modifiers, MouseButton, ParentElement, PinchEvent,
    Pixels, Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent, Size, Styled,
    TestAppContext, TouchPhase, VisualTestContext, Window, div, point, px, size,
};
use vespertine_components::media::ImageView;

fn natural() -> Size<Pixels> {
    size(px(1000.), px(500.))
}

fn viewport() -> Size<Pixels> {
    size(px(500.), px(300.))
}

fn f(p: Pixels) -> f32 {
    f32::from(p)
}

fn view_1000x500() -> ImageView {
    let view = ImageView::new();
    view.resize(natural(), viewport());
    view
}

#[test]
fn a1_fit_never_enlarges_and_centres() {
    let view = view_1000x500();
    assert_eq!(view.scale(), 0.5);
    assert_eq!(view.fit(), 0.5);
    let origin = view.image_origin();
    assert_eq!((f(origin.x), f(origin.y)), (0.0, 25.0));

    let small = ImageView::new();
    small.resize(size(px(100.), px(50.)), viewport());
    assert_eq!(small.scale(), 1.0);
    let origin = small.image_origin();
    assert_eq!((f(origin.x), f(origin.y)), (200.0, 125.0));
    assert!(small.is_fitted());
}

#[test]
fn invalid_sizes_are_ignored() {
    let view = view_1000x500();
    for bad in [f32::NAN, f32::INFINITY, 0.0, -3.0] {
        view.resize(size(px(bad), px(100.)), size(px(bad), px(bad)));
        assert_eq!(view.natural_size(), natural());
        assert_eq!(view.viewport_size(), viewport());
    }
}

#[test]
fn a2_zoom_keeps_the_anchored_source_point_and_clamps() {
    let view = view_1000x500();
    let anchor = point(px(100.), px(150.));
    let source_point = |view: &ImageView| {
        let pan = view.pan();
        let a = (100.0 - 250.0, 150.0 - 150.0);
        (
            (a.0 - f(pan.x)) / view.scale(),
            (a.1 - f(pan.y)) / view.scale(),
        )
    };
    let before = source_point(&view);
    view.zoom(2.0, anchor);
    assert_eq!(view.scale(), 2.0);
    assert!(!view.is_fitted());
    let after = source_point(&view);
    assert!((before.0 - after.0).abs() < 0.001 && (before.1 - after.1).abs() < 0.001);

    view.zoom(1e9, anchor);
    assert_eq!(view.scale(), 32.0);
    assert!(view.pan_by(point(px(1e9), px(-1e9))));
    let pan = view.pan();
    assert_eq!((f(pan.x), f(pan.y)), (15750.0, -7850.0));

    view.zoom(1e-9, anchor);
    assert_eq!(view.scale(), 0.01);
    let pan = view.pan();
    assert_eq!((f(pan.x), f(pan.y)), (0.0, 0.0));

    let scale = view.scale();
    let pan = view.pan();
    view.zoom(f32::NAN, anchor);
    view.zoom(2.0, point(px(f32::NAN), px(0.)));
    view.zoom(-1.0, anchor);
    view.zoom(0.0, anchor);
    assert_eq!(view.scale(), scale);
    assert_eq!(view.pan(), pan);
}

#[test]
fn a3_resize_refits_only_while_fitted() {
    let view = view_1000x500();
    view.set_viewport(size(px(250.), px(200.)));
    assert_eq!(view.scale(), 0.25);

    let view = view_1000x500();
    view.zoom(1.0, point(px(250.), px(150.)));
    assert_eq!(view.scale(), 1.0);
    view.set_viewport(size(px(300.), px(200.)));
    assert_eq!(view.scale(), 1.0);
    assert!(!view.is_fitted());
    assert_eq!(view.pan(), point(px(0.), px(0.)));
    // Panned to the limit, a larger viewport re-clamps: (1000 - 900) / 2 and (500 - 400) / 2.
    assert!(view.pan_by(point(px(1e6), px(1e6))));
    assert_eq!((f(view.pan().x), f(view.pan().y)), (350.0, 150.0));
    view.set_viewport(size(px(900.), px(400.)));
    assert_eq!(view.scale(), 1.0);
    assert_eq!((f(view.pan().x), f(view.pan().y)), (50.0, 50.0));
}

#[test]
fn zoom_at_most_scale_for_enormous_vectors_is_bounded() {
    let view = ImageView::new();
    view.resize(size(px(1_000_000.), px(1000.)), size(px(500.), px(300.)));
    view.zoom(1e9, point(px(0.), px(0.)));
    assert!((view.scale() - 131_072.0 / 1_000_000.0).abs() < 1e-6);
}

struct Host {
    view: ImageView,
    image: Arc<RenderImage>,
    clicks: Rc<Cell<usize>>,
    renders: Rc<Cell<usize>>,
    outer_clicks: Rc<Cell<usize>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        let clicks = self.clicks.clone();
        let outer = self.outer_clicks.clone();
        div()
            .size_full()
            .on_mouse_up(MouseButton::Left, move |_, _, _| outer.set(outer.get() + 1))
            .child(div().w(px(500.)).h(px(300.)).flex().child(self.view.render(
                self.image.clone(),
                natural(),
                Some(Rc::new(move |_, _, _| clicks.set(clicks.get() + 1))),
                None,
            )))
    }
}

struct Fixture {
    view: ImageView,
    clicks: Rc<Cell<usize>>,
    outer: Rc<Cell<usize>>,
}

fn setup(cx: &mut TestAppContext) -> (Fixture, &mut VisualTestContext) {
    let view = ImageView::new();
    let clicks = Rc::new(Cell::new(0));
    let outer = Rc::new(Cell::new(0));
    let (host_view, outer_clicks) = (view.clone(), outer.clone());
    let click_counter = clicks.clone();
    let (_, vcx) = cx.add_window_view(move |_, _| Host {
        view: host_view,
        image: solid_image(10, 5),
        clicks: click_counter,
        renders: Rc::new(Cell::new(0)),
        outer_clicks,
    });
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    (
        Fixture {
            view,
            clicks,
            outer,
        },
        vcx,
    )
}

fn wheel(cx: &mut VisualTestContext, position: Point<Pixels>, delta: ScrollDelta, control: bool) {
    cx.simulate_event(ScrollWheelEvent {
        position,
        delta,
        modifiers: Modifiers {
            control,
            ..Modifiers::none()
        },
        touch_phase: TouchPhase::Moved,
    });
}

#[vespertine::test]
fn a3_measured_viewport_refits_the_fitted_view(cx: &mut TestAppContext) {
    let (fixture, _cx) = setup(cx);
    assert_eq!(fixture.view.viewport_size(), viewport());
    assert_eq!(fixture.view.scale(), 0.5);
}

#[vespertine::test]
fn a4_wheel_pans_without_control_and_zooms_with_it(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx);
    let centre = point(px(250.), px(150.));
    // Fitted: a plain wheel line cannot pan and is not consumed.
    wheel(cx, centre, ScrollDelta::Lines(point(0., 1.)), false);
    assert_eq!(fixture.view.pan(), point(px(0.), px(0.)));
    // Control zoom: +1 line = 40 px increases the scale; the opposite delta returns it.
    let start = fixture.view.scale();
    wheel(cx, centre, ScrollDelta::Lines(point(0., 1.)), true);
    let zoomed = fixture.view.scale();
    assert!(zoomed > start);
    assert!((zoomed - start * (40.0f32 * 0.0025).exp()).abs() < 1e-4);
    wheel(
        cx,
        centre,
        ScrollDelta::Pixels(point(px(0.), px(-40.))),
        true,
    );
    assert!((fixture.view.scale() - start).abs() < 1e-4);
    // Zoomed in far enough to pan: the plain wheel pans and clamps.
    fixture.view.zoom(2.0, centre);
    let before = fixture.view.pan();
    wheel(cx, centre, ScrollDelta::Lines(point(0., 1.)), false);
    assert_eq!(f(fixture.view.pan().y), f(before.y) + 40.0);
}

#[vespertine::test]
fn a5_pinch_accumulates_and_end_clears_the_record(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx);
    let centre = point(px(250.), px(150.));
    let pinch = |cx: &mut VisualTestContext, phase, delta, position| {
        cx.simulate_event(PinchEvent {
            position,
            delta,
            modifiers: Modifiers::none(),
            phase,
        });
    };
    assert_eq!(fixture.view.scale(), 0.5);
    pinch(cx, TouchPhase::Started, 0.0, centre);
    pinch(cx, TouchPhase::Moved, 0.2, centre);
    pinch(cx, TouchPhase::Moved, 0.2, centre);
    assert!((fixture.view.scale() - 0.7).abs() < 1e-5);
    pinch(cx, TouchPhase::Moved, f32::NAN, centre);
    assert!((fixture.view.scale() - 0.7).abs() < 1e-5);
    pinch(cx, TouchPhase::Ended, 0.0, centre);
    // A new pinch starts from the current scale.
    pinch(cx, TouchPhase::Started, 0.0, centre);
    pinch(cx, TouchPhase::Moved, 0.5, centre);
    assert!((fixture.view.scale() - 0.7 * 1.5).abs() < 1e-4);
    pinch(cx, TouchPhase::Ended, 0.0, centre);
}

#[vespertine::test]
fn a5_pinch_at_a_corner_keeps_the_corners_source_point(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx);
    // At scale 1 the image is larger than the viewport, so the pan clamp does not interfere.
    fixture.view.zoom(1.0, point(px(250.), px(150.)));
    cx.run_until_parked();
    let corner = point(px(0.), px(0.));
    let source = |view: &ImageView| {
        let origin = view.image_origin();
        (-f(origin.x) / view.scale(), -f(origin.y) / view.scale())
    };
    let before = source(&fixture.view);
    for (phase, delta) in [
        (TouchPhase::Started, 0.0),
        (TouchPhase::Moved, 0.2),
        (TouchPhase::Ended, 0.0),
    ] {
        cx.simulate_event(PinchEvent {
            position: corner,
            delta,
            modifiers: Modifiers::none(),
            phase,
        });
    }
    let after = source(&fixture.view);
    assert!((fixture.view.scale() - 1.2).abs() < 1e-5);
    assert!((before.0 - after.0).abs() < 0.01 && (before.1 - after.1).abs() < 0.01);
}

#[vespertine::test]
fn a6_click_versus_drag(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx);
    // Zoomed in so a drag has room to pan.
    fixture.view.zoom(2.0, point(px(250.), px(150.)));
    cx.run_until_parked();
    let inside = point(px(250.), px(150.));
    let none = Modifiers::none();

    // A 2 px move after press does not drag.
    cx.simulate_mouse_down(inside, MouseButton::Left, none);
    cx.simulate_mouse_move(inside + point(px(2.), px(0.)), MouseButton::Left, none);
    assert!(!fixture.view.dragged());
    cx.simulate_mouse_up(inside + point(px(2.), px(0.)), MouseButton::Left, none);
    assert_eq!(
        fixture.clicks.get(),
        1,
        "a click inside delivers the callback exactly once"
    );
    assert_eq!(
        fixture.outer.get(),
        0,
        "a delivered click stops propagation"
    );

    // A 20 px move does drag, pans, and its click is swallowed.
    let pan_before = fixture.view.pan();
    cx.simulate_mouse_down(inside, MouseButton::Left, none);
    cx.simulate_mouse_move(inside + point(px(20.), px(0.)), MouseButton::Left, none);
    assert!(fixture.view.dragged());
    assert_eq!(f(fixture.view.pan().x), f(pan_before.x) + 20.0);
    cx.simulate_mouse_up(inside + point(px(20.), px(0.)), MouseButton::Left, none);
    assert_eq!(
        fixture.clicks.get(),
        1,
        "a click after a drag is not delivered"
    );
    assert_eq!(
        fixture.outer.get(),
        0,
        "a swallowed click stops propagation"
    );

    // The next press resets the dragged flag.
    cx.simulate_mouse_down(inside, MouseButton::Left, none);
    assert!(!fixture.view.dragged());
    cx.simulate_mouse_up(inside, MouseButton::Left, none);
    assert_eq!(fixture.clicks.get(), 2);
}

#[vespertine::test]
fn a6_a_move_without_the_button_clears_the_drag_record(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx);
    fixture.view.zoom(2.0, point(px(250.), px(150.)));
    let inside = point(px(250.), px(150.));
    let none = Modifiers::none();
    cx.simulate_mouse_down(inside, MouseButton::Left, none);
    cx.simulate_mouse_move(inside + point(px(30.), px(0.)), None, none);
    assert!(!fixture.view.dragged());
    cx.simulate_mouse_move(inside + point(px(60.), px(0.)), MouseButton::Left, none);
    assert!(
        !fixture.view.dragged(),
        "the record was cleared, so later moves do not drag"
    );
}

#[vespertine::test]
fn a6_a_click_outside_the_image_does_not_reach_the_callback(cx: &mut TestAppContext) {
    let (fixture, cx) = setup(cx);
    // Fitted at 0.5 the image occupies y 25..275; (250, 10) is in the viewport above it.
    cx.simulate_click(point(px(250.), px(10.)), Modifiers::none());
    assert_eq!(fixture.clicks.get(), 0);
    assert_eq!(fixture.outer.get(), 1, "an undelivered click propagates");
    cx.simulate_click(point(px(250.), px(150.)), Modifiers::none());
    assert_eq!(fixture.clicks.get(), 1);
}
