//! CMP-030: the elastic wrapper over a real scroll container (A7, A9, A10).

use std::{cell::Cell, rc::Rc};

use vespertine::{
    AppContext as _, Context, InteractiveElement, IntoElement, ParentElement, Render, ScrollDelta,
    ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement, Styled, TestAppContext, TouchPhase,
    Window, div, point, px,
};
use vespertine_components::elastic::{ScrollSource, elastic_scroll};

struct Fixture {
    handle: ScrollHandle,
    container_events: Rc<Cell<u32>>,
    reliable: Option<bool>,
    drag_source: bool,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let events = self.container_events.clone();
        let container = div()
            .id("container")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.handle)
            .on_scroll_wheel(move |_, _, _| events.set(events.get() + 1))
            .child(
                div()
                    .id("content")
                    .debug_selector(|| "content".into())
                    .flex_none()
                    .w_full()
                    .h(px(900.0)),
            );
        let mut wrapper = elastic_scroll(
            "elastic",
            ScrollSource::Handle(self.handle.clone()),
            container,
        );
        if let Some(reliable) = self.reliable {
            wrapper = wrapper.reliable_phases(reliable);
        }
        let mut root = div().w(px(300.0)).h(px(300.0)).child(wrapper);
        if self.drag_source {
            root = root.child(
                div()
                    .id("drag")
                    .debug_selector(|| "drag".into())
                    .absolute()
                    .left(px(300.0))
                    .w(px(40.0))
                    .h(px(40.0))
                    .on_drag((), |_, _, _, cx| cx.new(|_| Empty)),
            );
        }
        root
    }
}

struct Empty;

impl Render for Empty {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(1.0))
    }
}

struct Harness {
    handle: ScrollHandle,
    events: Rc<Cell<u32>>,
}

fn wheel(cx: &mut vespertine::VisualTestContext, phase: TouchPhase, dy: f32, dx: f32) {
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(150.0), px(150.0)),
        delta: ScrollDelta::Pixels(point(px(dx), px(dy))),
        touch_phase: phase,
        ..Default::default()
    });
}

/// Draws the root view once (the window redraws it after every event).
fn redraw(cx: &mut vespertine::VisualTestContext) {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
}

fn setup(
    cx: &mut TestAppContext,
    reliable: Option<bool>,
    drag_source: bool,
) -> (Harness, &mut vespertine::VisualTestContext) {
    let handle = ScrollHandle::new();
    let events = Rc::new(Cell::new(0));
    let fixture = Fixture {
        handle: handle.clone(),
        container_events: events.clone(),
        reliable,
        drag_source,
    };
    let (_view, cx) = cx.add_window_view(|_, _| fixture);
    redraw(cx);
    (Harness { handle, events }, cx)
}

fn content_top(cx: &mut vespertine::VisualTestContext) -> f32 {
    f32::from(cx.debug_bounds("content").expect("content bounds").origin.y)
}

#[vespertine::test]
fn a9_the_container_stays_clamped_while_the_content_shifts_by_the_band(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, Some(true), false);
    assert_eq!(content_top(cx), 0.0);
    wheel(cx, TouchPhase::Started, 0.0, 0.0);
    // An outward delta at the top: propagates (the container's own handler still fires).
    let before = h.events.get();
    wheel(cx, TouchPhase::Moved, 60.0, 0.0);
    eprintln!("events {} -> {}", before, h.events.get());
    assert_eq!(
        h.events.get(),
        before + 1,
        "outward events reach the container"
    );
    assert_eq!(
        h.handle.offset().y,
        px(0.0),
        "the container offset stays clamped"
    );
    redraw(cx);
    let shift = content_top(cx);
    assert!(
        shift > 0.0 && shift < 60.0,
        "the content shifts by the band: {shift}"
    );
    // An inward delta the band absorbs is consumed: the container does not see it.
    let seen = h.events.get();
    wheel(cx, TouchPhase::Moved, -20.0, 0.0);
    assert_eq!(
        h.events.get(),
        seen,
        "absorbed inward input is not observed"
    );
    redraw(cx);
    assert!(content_top(cx) < shift);
    // An inward swipe larger than the band collapses it and reaches the container whole.
    wheel(cx, TouchPhase::Moved, -100.0, 0.0);
    assert_eq!(h.events.get(), seen + 1);
    assert_eq!(
        h.handle.offset().y,
        px(-100.0),
        "the unconsumed delta scrolled the container"
    );
    redraw(cx);
    assert_eq!(content_top(cx), -100.0);
}

#[vespertine::test]
fn a10_linux_default_wrapper_neither_consumes_nor_reshapes_events(cx: &mut TestAppContext) {
    // No capability flag and no override: the explicit clamped fallback.
    let (h, cx) = setup(cx, None, false);
    wheel(cx, TouchPhase::Started, 0.0, 0.0);
    for delta in [60.0, -20.0, 40.0] {
        let before = h.events.get();
        wheel(cx, TouchPhase::Moved, delta, 0.0);
        assert_eq!(
            h.events.get(),
            before + 1,
            "every event reaches the container"
        );
    }
    wheel(cx, TouchPhase::Ended, 0.0, 0.0);
    redraw(cx);
    assert_eq!(
        content_top(cx),
        f32::from(h.handle.offset().y),
        "no band offset"
    );
}

#[vespertine::test]
fn a7_reduced_motion_and_an_explicit_disable_keep_the_wrapper_inert(cx: &mut TestAppContext) {
    let (h, cx) = setup(cx, Some(true), false);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    redraw(cx);
    wheel(cx, TouchPhase::Started, 0.0, 0.0);
    let before = h.events.get();
    wheel(cx, TouchPhase::Moved, 80.0, 0.0);
    wheel(cx, TouchPhase::Ended, 0.0, 0.0);
    redraw(cx);
    assert_eq!(
        content_top(cx),
        0.0,
        "reduced motion: no displacement for any phase stream"
    );
    assert_eq!(
        h.events.get(),
        before + 2,
        "every event reaches the container"
    );
    cx.update(|_, cx| cx.set_reduce_motion(false));
}

#[vespertine::test]
fn a7_an_active_drag_disables_the_wrapper(cx: &mut TestAppContext) {
    let (_h, cx) = setup(cx, Some(true), true);
    let source = cx.debug_bounds("drag").expect("drag source");
    // Start a drag-and-drop: press and move beyond the drag threshold.
    cx.simulate_mouse_down(
        source.center(),
        vespertine::MouseButton::Left,
        Default::default(),
    );
    cx.simulate_mouse_move(
        source.center() + point(px(30.0), px(30.0)),
        vespertine::MouseButton::Left,
        Default::default(),
    );
    assert!(cx.update(|_, cx| cx.has_active_drag()), "a drag is active");
    redraw(cx);
    wheel(cx, TouchPhase::Started, 0.0, 0.0);
    wheel(cx, TouchPhase::Moved, 80.0, 0.0);
    redraw(cx);
    assert_eq!(content_top(cx), 0.0, "an active drag disables the band");
}
