//! CMP-020 A7 (the shared full view) and the inline preview (R8).

mod media_support;

use media_support::solid_image;
use std::cell::Cell;
use std::rc::Rc;
use vespertine::{
    AnyView, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, Modifiers,
    MouseButton, ParentElement, PinchEvent, Pixels, Point, Render, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement, StyleRefinement, Styled, TestAppContext, TouchPhase,
    VisualTestContext, Window, div, point, px, size,
};
use vespertine_components::media::{ImageView, PreviewTarget, lightbox};

struct Sibling {
    renders: Rc<Cell<usize>>,
}

impl Render for Sibling {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div().w(px(10.)).h(px(10.))
    }
}

struct Root {
    open: bool,
    view: ImageView,
    focus: FocusHandle,
    sibling: Entity<Sibling>,
    loaded: bool,
    closes: Rc<Cell<usize>>,
    beneath_clicks: Rc<Cell<usize>>,
    beneath_scrolls: Rc<Cell<usize>>,
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let closes = self.closes.clone();
        let beneath = self.beneath_clicks.clone();
        let scrolls = self.beneath_scrolls.clone();
        let mut root = div()
            .size_full()
            .on_scroll_wheel(move |_, _, _| scrolls.set(scrolls.get() + 1))
            .child(
                div()
                    .id("beneath")
                    .w(px(1000.))
                    .h(px(700.))
                    .on_click(move |_, _, _| beneath.set(beneath.get() + 1)),
            )
            .child(AnyView::from(self.sibling.clone()).cached(StyleRefinement::default()));
        if self.open {
            let target = PreviewTarget {
                name: "diagram.png".into(),
                image: self.loaded.then(|| solid_image(40, 20)),
                natural_size: self.loaded.then(|| size(px(400.), px(200.))),
                view: self.view.clone(),
                plate: None,
            };
            root = root.child(lightbox(window, target, &self.focus, move |_, cx| {
                closes.set(closes.get() + 1);
                entity.update(cx, |root, cx| {
                    root.open = false;
                    cx.notify();
                });
            }));
        }
        root
    }
}

struct Fixture {
    root: Entity<Root>,
    view: ImageView,
    sibling_renders: Rc<Cell<usize>>,
    closes: Rc<Cell<usize>>,
    beneath_clicks: Rc<Cell<usize>>,
    beneath_scrolls: Rc<Cell<usize>>,
}

fn setup(cx: &mut TestAppContext, loaded: bool) -> (Fixture, &mut VisualTestContext) {
    let view = ImageView::new();
    let sibling_renders = Rc::new(Cell::new(0));
    let closes = Rc::new(Cell::new(0));
    let beneath_clicks = Rc::new(Cell::new(0));
    let beneath_scrolls = Rc::new(Cell::new(0));
    let (root, vcx) = cx.add_window_view({
        let view = view.clone();
        let sibling_renders = sibling_renders.clone();
        let closes = closes.clone();
        let beneath_clicks = beneath_clicks.clone();
        let beneath_scrolls = beneath_scrolls.clone();
        move |_, cx| Root {
            open: true,
            view,
            focus: cx.focus_handle(),
            sibling: cx.new(|_| Sibling {
                renders: sibling_renders,
            }),
            loaded,
            closes,
            beneath_clicks,
            beneath_scrolls,
        }
    });
    vcx.run_until_parked();
    vcx.update(|window, _| window.refresh());
    vcx.run_until_parked();
    (
        Fixture {
            root,
            view,
            sibling_renders,
            closes,
            beneath_clicks,
            beneath_scrolls,
        },
        vcx,
    )
}

fn is_open(fixture: &Fixture, cx: &mut VisualTestContext) -> bool {
    fixture.root.read_with(cx, |root, _| root.open)
}

fn centre(cx: &mut VisualTestContext) -> Point<Pixels> {
    let size = cx.update(|window, _| window.viewport_size());
    point(size.width / 2.0, size.height / 2.0)
}

#[vespertine::test]
fn a7_layout_is_90_by_85_percent_with_a_caption_and_loading_text(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, true);
    let window = vcx.update(|window, _| window.viewport_size());
    let viewer = fixture.view.viewport_size();
    assert!((f32::from(viewer.width) - f32::from(window.width) * 0.9).abs() < 1.0);
    assert!((f32::from(viewer.height) - f32::from(window.height) * 0.85).abs() < 1.0);
    let caption = vcx.debug_bounds("lightbox-caption").expect("caption");
    assert!(f32::from(caption.size.width) <= f32::from(window.width) * 0.9 + 0.5);
    assert!(vcx.debug_bounds("lightbox-loading").is_none());

    let (_, loading) = setup(cx, false);
    assert!(
        loading.debug_bounds("lightbox-loading").is_some(),
        "Loading image shows instead of the viewer"
    );
}

#[vespertine::test]
fn a7_escape_closes(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, true);
    assert!(is_open(&fixture, vcx));
    vcx.simulate_keystrokes("escape");
    assert!(!is_open(&fixture, vcx));
    assert_eq!(fixture.closes.get(), 1);
}

#[vespertine::test]
fn a7_a_plain_click_closes_without_reaching_the_content_beneath(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, true);
    // On the image itself.
    let middle = centre(vcx);
    vcx.simulate_click(middle, Modifiers::none());
    assert!(!is_open(&fixture, vcx));
    assert_eq!(
        fixture.beneath_clicks.get(),
        0,
        "the overlay occludes the content beneath"
    );

    // On the bare scrim.
    let (fixture, vcx) = setup(cx, true);
    vcx.simulate_click(point(px(3.), px(3.)), Modifiers::none());
    assert!(!is_open(&fixture, vcx));
    assert_eq!(fixture.beneath_clicks.get(), 0);
}

#[vespertine::test]
fn a7_zoom_pinch_and_drag_do_not_close_and_the_wheel_is_consumed(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, true);
    let middle = centre(vcx);
    let scale = fixture.view.scale();
    vcx.simulate_event(ScrollWheelEvent {
        position: middle,
        delta: ScrollDelta::Lines(point(0., 3.)),
        modifiers: Modifiers {
            control: true,
            ..Modifiers::none()
        },
        touch_phase: TouchPhase::Moved,
    });
    assert!(fixture.view.scale() > scale, "control-wheel zooms");
    assert!(is_open(&fixture, vcx));
    // A plain wheel over the scrim never reaches the page beneath.
    vcx.simulate_event(ScrollWheelEvent {
        position: point(px(3.), px(3.)),
        delta: ScrollDelta::Lines(point(0., 3.)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    assert_eq!(fixture.beneath_scrolls.get(), 0);
    assert!(is_open(&fixture, vcx));

    let zoomed = fixture.view.scale();
    for (phase, delta) in [
        (TouchPhase::Started, 0.0),
        (TouchPhase::Moved, 0.3),
        (TouchPhase::Ended, 0.0),
    ] {
        vcx.simulate_event(PinchEvent {
            position: middle,
            delta,
            modifiers: Modifiers::none(),
            phase,
        });
    }
    assert!(fixture.view.scale() > zoomed);
    assert!(is_open(&fixture, vcx));

    // A 50 px drag on the image.
    let none = Modifiers::none();
    vcx.simulate_mouse_down(middle, MouseButton::Left, none);
    vcx.simulate_mouse_move(middle + point(px(50.), px(0.)), MouseButton::Left, none);
    vcx.simulate_mouse_up(middle + point(px(50.), px(0.)), MouseButton::Left, none);
    assert!(is_open(&fixture, vcx), "a drag never closes");
    assert_eq!(fixture.closes.get(), 0);
}

#[vespertine::test]
fn a7_a_drag_that_ends_on_the_scrim_does_not_close(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, true);
    let middle = centre(vcx);
    let none = Modifiers::none();
    // Zoom in so the drag can pan, then drag from the image to the far corner scrim.
    fixture.view.zoom(2.0, point(px(100.), px(100.)));
    vcx.simulate_mouse_down(middle, MouseButton::Left, none);
    vcx.simulate_mouse_move(point(px(2.), px(2.)), MouseButton::Left, none);
    vcx.simulate_mouse_up(point(px(2.), px(2.)), MouseButton::Left, none);
    assert!(fixture.view.dragged());
    assert!(
        is_open(&fixture, vcx),
        "a drag ending on the scrim does not close it"
    );

    // A following plain click on the scrim starts a fresh decision and closes.
    vcx.simulate_click(point(px(3.), px(3.)), Modifiers::none());
    assert!(!is_open(&fixture, vcx));
}

#[vespertine::test]
fn a7_zoom_activity_does_not_rerender_an_unrelated_cached_sibling(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, true);
    let before = fixture.sibling_renders.get();
    assert!(before >= 1);
    let middle = centre(vcx);
    for _ in 0..5 {
        vcx.simulate_event(ScrollWheelEvent {
            position: middle,
            delta: ScrollDelta::Lines(point(0., 1.)),
            modifiers: Modifiers {
                control: true,
                ..Modifiers::none()
            },
            touch_phase: TouchPhase::Moved,
        });
    }
    assert!(fixture.view.scale() > 0.0);
    assert_eq!(
        fixture.sibling_renders.get(),
        before,
        "the cached sibling was not re-rendered"
    );
}

#[vespertine::test]
fn a7_reopening_resets_the_viewer_to_fit(cx: &mut TestAppContext) {
    let (fixture, vcx) = setup(cx, true);
    let middle = centre(vcx);
    fixture.view.zoom(3.0, middle);
    vcx.simulate_mouse_down(middle, MouseButton::Left, Modifiers::none());
    vcx.simulate_mouse_move(
        middle + point(px(40.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    assert!(fixture.view.dragged());
    fixture.view.reset();
    assert!(fixture.view.is_fitted());
    assert!(!fixture.view.dragged());
    assert_eq!(fixture.view.pan(), point(px(0.), px(0.)));
}
