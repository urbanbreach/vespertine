//! LAY-005 acceptance tests that run real element trees in a headless window.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use super::*;
use crate::{
    AppContext as _, Context, CustomElementRegistry, Hsla, MouseButton, MouseDownEvent,
    PlatformInput, Render, Rgba, TestAppContext, WindowHandle, canvas, size, transparent_black,
};

type Build = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

struct FloatView {
    build: Build,
}

impl Render for FloatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.build)(window, cx)
    }
}

fn open(
    cx: &mut TestAppContext,
    window_size: Size<Pixels>,
    build: impl Fn(&mut Window) -> AnyElement + 'static,
) -> WindowHandle<FloatView> {
    open_with_app(cx, window_size, move |window, _| build(window))
}

fn open_with_app(
    cx: &mut TestAppContext,
    window_size: Size<Pixels>,
    build: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
) -> WindowHandle<FloatView> {
    let build: Build = Rc::new(build);
    let window = cx.open_window(window_size, move |_, _| FloatView { build });
    redraw(cx, window);
    window
}

fn redraw(cx: &mut TestAppContext, window: WindowHandle<FloatView>) {
    window.update(cx, |_, _, cx| cx.notify()).unwrap();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

/// The painted solid quads, in paint order, in logical pixels.
fn painted_quads(
    cx: &mut TestAppContext,
    window: WindowHandle<FloatView>,
) -> Vec<(Bounds<Pixels>, Rgba)> {
    window
        .update(cx, |_, window, _| {
            let scale = window.scale_factor();
            window
                .rendered_frame
                .scene
                .quads
                .iter()
                .map(|quad| {
                    let bounds = Bounds::new(
                        point(
                            px(quad.bounds.origin.x.0 / scale),
                            px(quad.bounds.origin.y.0 / scale),
                        ),
                        size(
                            px(quad.bounds.size.width.0 / scale),
                            px(quad.bounds.size.height.0 / scale),
                        ),
                    );
                    (bounds, Rgba::from(quad.background.solid))
                })
                .collect()
        })
        .unwrap()
}

fn close(a: Rgba, b: Rgba) -> bool {
    (a.r - b.r).abs() < 0.01
        && (a.g - b.g).abs() < 0.01
        && (a.b - b.b).abs() < 0.01
        && (a.a - b.a).abs() < 0.01
}

fn fallback() -> Rgba {
    rgb(FALLBACK_BACKGROUND)
}

/// A gray 80x30 trigger at (100, 60) owning one layer built by `layer`.
fn trigger_with(layer: impl FnOnce() -> FloatingLayer) -> AnyElement {
    div()
        .size_full()
        .bg(rgb(0xFFFFFF))
        .child(
            div()
                .absolute()
                .left(px(100.))
                .top(px(60.))
                .w(px(80.))
                .h(px(30.))
                .bg(rgb(0x808080))
                .child(layer()),
        )
        .into_any_element()
}

fn layer_bounds(cx: &mut TestAppContext, window: WindowHandle<FloatView>) -> Vec<Bounds<Pixels>> {
    painted_quads(cx, window)
        .into_iter()
        .filter(|(_, color)| close(*color, fallback()))
        .map(|(bounds, _)| bounds)
        .collect()
}

#[vespertine::test]
fn fidelity_scene_layer_is_at_80_96_and_reports_its_final_bounds(cx: &mut TestAppContext) {
    let tracker = BoundsTracker::new();
    let window = open(cx, size(px(320.), px(200.)), {
        let tracker = tracker.clone();
        move |_| {
            trigger_with(|| {
                floating_layer("layer")
                    .side(FloatingSide::Bottom)
                    .align(FloatingAlign::End)
                    .gap(px(6.))
                    .bounds_tracker(&tracker)
                    .w(px(100.))
                    .h(px(50.))
            })
        }
    });
    let expected = Bounds::new(point(px(80.), px(96.)), size(px(100.), px(50.)));
    assert_eq!(tracker.get(), Some(expected));
    assert_eq!(layer_bounds(cx, window), vec![expected]);
    // The layer occupies no flow space: the trigger keeps its own box.
    let quads = painted_quads(cx, window);
    assert!(quads.iter().any(|(bounds, color)| {
        *bounds == Bounds::new(point(px(100.), px(60.)), size(px(80.), px(30.)))
            && close(*color, rgb(0x808080))
    }));
}

#[vespertine::test]
fn a1_every_side_and_alignment_painted_bounds_equal_recorded_and_computed(cx: &mut TestAppContext) {
    let window_size = size(px(400.), px(300.));
    let content = size(px(60.), px(40.));
    for side in [
        FloatingSide::Top,
        FloatingSide::Right,
        FloatingSide::Bottom,
        FloatingSide::Left,
    ] {
        for align in [
            FloatingAlign::Start,
            FloatingAlign::Center,
            FloatingAlign::End,
        ] {
            let tracker = BoundsTracker::new();
            let painted = Rc::new(Cell::new(None));
            let window = open(cx, window_size, {
                let tracker = tracker.clone();
                let painted = painted.clone();
                move |_| {
                    let painted = painted.clone();
                    div()
                        .size_full()
                        .child(
                            div()
                                .absolute()
                                .left(px(170.))
                                .top(px(130.))
                                .w(px(80.))
                                .h(px(30.))
                                .child(
                                    floating_layer("layer")
                                        .side(side)
                                        .align(align)
                                        .gap(px(6.))
                                        .bounds_tracker(&tracker)
                                        .w(content.width)
                                        .h(content.height)
                                        .child(
                                            canvas(
                                                |_, _, _| (),
                                                move |bounds, _, _, _| painted.set(Some(bounds)),
                                            )
                                            .size_full(),
                                        ),
                                ),
                        )
                        .into_any_element()
                }
            });
            let trigger = Bounds::new(point(px(170.), px(130.)), size(px(80.), px(30.)));
            let options = FloatingOptions {
                side,
                align,
                gap: px(6.),
                ..Default::default()
            };
            let computed = place_floating(&options, trigger, None, content, window_size, px(0.));
            let recorded = tracker.get().expect("layer painted");
            assert_eq!(recorded, computed, "{side:?} {align:?}");
            assert_eq!(
                painted.get(),
                Some(recorded),
                "{side:?} {align:?} painted vs recorded"
            );
            assert_eq!(
                layer_bounds(cx, window),
                vec![recorded],
                "{side:?} {align:?}"
            );
        }
    }
}

#[vespertine::test]
fn a2_explicit_position_replaces_the_trigger_attach_point(cx: &mut TestAppContext) {
    let tracker = BoundsTracker::new();
    open(cx, size(px(320.), px(200.)), {
        let tracker = tracker.clone();
        move |_| {
            trigger_with(|| {
                floating_layer("layer")
                    .position(point(px(200.), px(120.)))
                    .anchor(Anchor::BottomRight)
                    .side(FloatingSide::Right)
                    .gap(px(4.))
                    .offset(point(px(-10.), px(2.)))
                    .bounds_tracker(&tracker)
                    .w(px(50.))
                    .h(px(30.))
            })
        }
    });
    // (200,120) + gap 4 right + offset (-10,2) = (194,122) is the bottom-right corner.
    assert_eq!(
        tracker.get(),
        Some(Bounds::new(
            point(px(144.), px(92.)),
            size(px(50.), px(30.))
        ))
    );
}

#[vespertine::test]
fn a2_snapping_with_margin_on_every_edge_and_wider_than_window(cx: &mut TestAppContext) {
    let window_size = size(px(320.), px(200.));
    let cases = [
        // (position, content size, expected origin)
        ((300., 20.), (100., 50.), (212., 20.)), // right shifts left
        ((20., 190.), (100., 50.), (20., 142.)), // bottom shifts up
        ((-40., 20.), (100., 50.), (8., 20.)),   // left clamps to the margin
        ((20., -40.), (100., 50.), (20., 8.)),   // top clamps to the margin
        ((50., 20.), (500., 50.), (8., 20.)),    // wider than the window aligns left
    ];
    for (position, content, origin) in cases {
        let tracker = BoundsTracker::new();
        open(cx, window_size, {
            let tracker = tracker.clone();
            move |_| {
                trigger_with(|| {
                    floating_layer("layer")
                        .position(point(px(position.0), px(position.1)))
                        .bounds_tracker(&tracker)
                        .w(px(content.0))
                        .h(px(content.1))
                })
            }
        });
        assert_eq!(
            tracker.get().map(|bounds| bounds.origin),
            Some(point(px(origin.0), px(origin.1))),
            "position {position:?} content {content:?}"
        );
    }
}

#[vespertine::test]
fn a2_fallback_fill_and_transparent_opt_out(cx: &mut TestAppContext) {
    let build_with = |style: fn(FloatingLayer) -> FloatingLayer| {
        move |_: &mut Window| trigger_with(|| style(floating_layer("layer").w(px(60.)).h(px(40.))))
    };
    let default_fill = open(cx, size(px(320.), px(200.)), build_with(|layer| layer));
    assert_eq!(
        layer_bounds(cx, default_fill).len(),
        1,
        "fallback #1A1A1A fill painted"
    );

    let transparent = open(
        cx,
        size(px(320.), px(200.)),
        build_with(|layer| layer.bg(transparent_black())),
    );
    assert!(
        painted_quads(cx, transparent)
            .iter()
            .all(|(bounds, _)| bounds.size != size(px(60.), px(40.))),
        "explicit transparent background paints nothing for the fill"
    );

    let explicit = open(
        cx,
        size(px(320.), px(200.)),
        build_with(|layer| layer.bg(rgb(0xFF0000))),
    );
    let quads = painted_quads(cx, explicit);
    assert!(quads.iter().any(|(_, color)| close(*color, rgb(0xFF0000))));
    assert!(quads.iter().all(|(_, color)| !close(*color, fallback())));
}

#[vespertine::test]
fn a2_transparent_full_window_fill_paints_nothing_and_tracks_viewport_resize(
    cx: &mut TestAppContext,
) {
    let tracker = BoundsTracker::new();
    let window = open(cx, size(px(320.), px(200.)), {
        let tracker = tracker.clone();
        move |_| {
            trigger_with(|| {
                floating_layer("overlay")
                    .fill_window()
                    .bounds_tracker(&tracker)
                    .bg(transparent_black())
                    // Explicit style size must be overridden by the full-window fill.
                    .w(px(10.))
                    .h(px(10.))
            })
        }
    });
    assert_eq!(
        tracker.get(),
        Some(Bounds::new(point(px(0.), px(0.)), size(px(320.), px(200.))))
    );
    // Only the white page and the trigger paint; the transparent overlay adds nothing.
    assert_eq!(painted_quads(cx, window).len(), 2);

    // Resizing the window updates the layer in the same frame, without any polling.
    cx.simulate_window_resize(window.into(), size(px(480.), px(260.)));
    redraw(cx, window);
    assert_eq!(
        tracker.get(),
        Some(Bounds::new(point(px(0.), px(0.)), size(px(480.), px(260.))))
    );
}

#[vespertine::test]
fn a2_opaque_full_window_fill_covers_the_viewport(cx: &mut TestAppContext) {
    let window = open(cx, size(px(320.), px(200.)), |_| {
        trigger_with(|| {
            floating_layer("overlay")
                .fill_window()
                .gap(px(30.))
                .offset(point(px(9.), px(9.)))
        })
    });
    assert_eq!(
        layer_bounds(cx, window),
        vec![Bounds::new(point(px(0.), px(0.)), size(px(320.), px(200.)))]
    );
}

fn mouse_down(window: WindowHandle<FloatView>, cx: &mut TestAppContext, position: Point<Pixels>) {
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.dispatch_event(
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
    })
    .unwrap();
}

#[vespertine::test]
fn a2_occlusion_blocks_pointer_hits_beneath_by_default_and_can_be_disabled(
    cx: &mut TestAppContext,
) {
    for occlude in [true, false] {
        let hits = Rc::new(Cell::new(0));
        let window = open(cx, size(px(320.), px(200.)), {
            let hits = hits.clone();
            move |_| {
                let hits = hits.clone();
                div()
                    .size_full()
                    .child(
                        div()
                            .id("under")
                            .absolute()
                            .left(px(0.))
                            .top(px(100.))
                            .w(px(320.))
                            .h(px(100.))
                            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                                hits.set(hits.get() + 1)
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(100.))
                            .top(px(60.))
                            .w(px(80.))
                            .h(px(30.))
                            .child(
                                floating_layer("layer")
                                    .occlude_below(occlude)
                                    .w(px(100.))
                                    .h(px(80.))
                                    .child(div().size_full()),
                            ),
                    )
                    .into_any_element()
            }
        });
        // (150, 120) is inside both the layer (y 90..170) and the element beneath.
        mouse_down(window, cx, point(px(150.), px(120.)));
        assert_eq!(
            hits.get(),
            if occlude { 0 } else { 1 },
            "occlude = {occlude}"
        );
        // Outside the layer the element beneath is reachable either way.
        mouse_down(window, cx, point(px(10.), px(150.)));
        assert_eq!(
            hits.get(),
            if occlude { 1 } else { 2 },
            "occlude = {occlude}"
        );
    }
}

#[vespertine::test]
fn a2_deferred_layers_paint_by_priority_and_without_deferral_in_tree_order(
    cx: &mut TestAppContext,
) {
    let red: Hsla = rgb(0xFF0000).into();
    let blue: Hsla = rgb(0x0000FF).into();
    let order_of = |cx: &mut TestAppContext, window: WindowHandle<FloatView>| {
        painted_quads(cx, window)
            .into_iter()
            .filter(|(bounds, _)| bounds.size == size(px(60.), px(40.)))
            .map(|(_, color)| {
                if close(color, red.into()) {
                    "red"
                } else {
                    "blue"
                }
            })
            .collect::<Vec<_>>()
    };
    // Two overlapping layers, the first declared having the higher priority.
    let window = open(cx, size(px(320.), px(200.)), move |_| {
        div()
            .size_full()
            .child(
                floating_layer("high")
                    .position(point(px(50.), px(50.)))
                    .priority(2)
                    .bg(red)
                    .w(px(60.))
                    .h(px(40.)),
            )
            .child(
                floating_layer("low")
                    .position(point(px(70.), px(60.)))
                    .priority(1)
                    .bg(blue)
                    .w(px(60.))
                    .h(px(40.)),
            )
            .into_any_element()
    });
    assert_eq!(
        order_of(cx, window),
        vec!["blue", "red"],
        "higher priority paints on top"
    );

    // Without deferral both paint in tree order.
    let window = open(cx, size(px(320.), px(200.)), move |_| {
        div()
            .size_full()
            .child(
                floating_layer("high")
                    .position(point(px(50.), px(50.)))
                    .deferred(false)
                    .priority(2)
                    .bg(red)
                    .w(px(60.))
                    .h(px(40.)),
            )
            .child(
                floating_layer("low")
                    .position(point(px(70.), px(60.)))
                    .deferred(false)
                    .priority(1)
                    .bg(blue)
                    .w(px(60.))
                    .h(px(40.)),
            )
            .into_any_element()
    });
    assert_eq!(order_of(cx, window), vec!["red", "blue"]);
}

#[vespertine::test]
fn a2_switch_fitting_flips_the_content_anchor_when_it_overflows(cx: &mut TestAppContext) {
    let tracker = BoundsTracker::new();
    open(cx, size(px(320.), px(200.)), {
        let tracker = tracker.clone();
        move |_| {
            trigger_with(|| {
                floating_layer("layer")
                    .position(point(px(300.), px(20.)))
                    .fit(FloatingFit::Switch)
                    .bounds_tracker(&tracker)
                    .w(px(100.))
                    .h(px(50.))
            })
        }
    });
    // Top-left at x=300 overflows the 320 wide window; the switched top-right fits.
    assert_eq!(
        tracker.get().map(|bounds| bounds.origin),
        Some(point(px(200.), px(20.)))
    );
}

#[vespertine::test]
fn default_snap_margin_is_eight_on_every_edge(cx: &mut TestAppContext) {
    // A layer placed at the bottom-right corner keeps an 8px margin on both edges.
    let tracker = BoundsTracker::new();
    open(cx, size(px(320.), px(200.)), {
        let tracker = tracker.clone();
        move |_| {
            trigger_with(|| {
                floating_layer("layer")
                    .position(point(px(315.), px(195.)))
                    .bounds_tracker(&tracker)
                    .w(px(60.))
                    .h(px(40.))
            })
        }
    });
    let bounds = tracker.get().unwrap();
    assert_eq!(bounds.right(), px(312.));
    assert_eq!(bounds.bottom(), px(192.));
}

#[vespertine::test]
fn anchored_adapter_places_from_properties_and_forwards_standard_events(cx: &mut TestAppContext) {
    let registry = Rc::new(RefCell::new(CustomElementRegistry::default()));
    assert!(registry.borrow().has_factory("anchored"));
    let emitted: Rc<RefCell<Vec<crate::HostEvent>>> = Rc::default();

    let window = open_with_app(cx, size(px(320.), px(200.)), {
        let registry = registry.clone();
        let emitted = emitted.clone();
        move |window, cx| {
            let emitted = emitted.clone();
            let emit: crate::EmitEvent =
                Rc::new(move |event, _, _| emitted.borrow_mut().push(event));
            let mut context = RenderContext::new(HostId(1), emit);
            context.properties = Arc::new(
                serde_json::json!({"side": "bottom", "align": "end", "gap": 6, "future": 1})
                    .as_object()
                    .cloned()
                    .unwrap(),
            );
            // `keyDown` is not declared by the anchored adapter and is filtered out.
            context.events = ["click", "enter", "keyDown"]
                .map(Into::into)
                .into_iter()
                .collect();
            context.children = vec![div().w(px(100.)).h(px(50.)).into_any_element()];
            let layer = registry
                .borrow_mut()
                .render("anchored", context, window, cx);
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px(100.))
                        .top(px(60.))
                        .w(px(80.))
                        .h(px(30.))
                        .bg(rgb(0x808080))
                        .child(layer),
                )
                .into_any_element()
        }
    });
    assert_eq!(
        registry.borrow().last_bounds(HostId(1)),
        Some(Bounds::new(
            point(px(80.), px(96.)),
            size(px(100.), px(50.))
        ))
    );
    assert_eq!(layer_bounds(cx, window).len(), 1, "fallback fill painted");

    let inside = point(px(120.), px(110.));
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window.simulate_mouse_move(inside, cx);
        for event in [
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position: inside,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }),
            PlatformInput::MouseUp(crate::MouseUpEvent {
                button: MouseButton::Left,
                position: inside,
                modifiers: Default::default(),
                click_count: 1,
            }),
        ] {
            window.dispatch_event(event, cx);
        }
    })
    .unwrap();
    let names: Vec<_> = emitted
        .borrow()
        .iter()
        .map(|event| event.name().to_owned())
        .collect();
    assert_eq!(names, vec!["enter", "click"]);
}

#[vespertine::test]
fn layer_attaches_to_the_trigger_border_box_even_with_borders_and_padding(cx: &mut TestAppContext) {
    let tracker = BoundsTracker::new();
    open(cx, size(px(320.), px(200.)), {
        let tracker = tracker.clone();
        move |_| {
            div()
                .size_full()
                .child(
                    div()
                        .absolute()
                        .left(px(100.))
                        .top(px(60.))
                        .w(px(80.))
                        .h(px(30.))
                        .border_2()
                        .border_color(rgb(0x000000))
                        .p(px(5.))
                        .child(
                            floating_layer("layer")
                                .side(FloatingSide::Bottom)
                                .align(FloatingAlign::End)
                                .trigger_border(Edges::all(px(2.)))
                                .bounds_tracker(&tracker)
                                .w(px(40.))
                                .h(px(20.)),
                        ),
                )
                .into_any_element()
        }
    });
    // The trigger border box is (100,60,80,30): bottom-end attach (180,90), content top-right.
    assert_eq!(
        tracker.get(),
        Some(Bounds::new(
            point(px(140.), px(90.)),
            size(px(40.), px(20.))
        ))
    );
}
