//! CMP-030 A1: a touch fling bounces, a phase-less mouse wheel does not, and an active drag
//! disables the wrapper.
//!
//! The example opens an elastic scroll container (enabled per wrapper, as a host with reliable
//! gesture phases would) and runs three scripted input streams against its top and bottom ends:
//! a touch fling (`Started`, drag, `Ended`, momentum events), a phase-less mouse wheel and the
//! same touch stream during an active drag-and-drop. The observed peak band offsets are shown on
//! screen and printed; with `ELASTIC_SCRIPT=1` the example exits 0 when bounce, no bounce and no
//! band are observed respectively (1 otherwise).

use std::{cell::RefCell, rc::Rc, time::Duration};

use vespertine::{
    App, AppContext as _, Bounds, Context, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PlatformInput, Render,
    ScrollDelta, ScrollHandle, ScrollWheelEvent, StatefulInteractiveElement, Styled, TouchPhase,
    Window, WindowBounds, WindowOptions, div, point, px, rgb, size,
};
use vespertine_components::elastic::{ReliableScrollPhases, ScrollSource, elastic_scroll};
use vespertine_platform::application;

#[derive(Default)]
struct Peaks {
    current: f32,
    top: f32,
    bottom: f32,
}

struct Example {
    handle: ScrollHandle,
    peaks: Rc<RefCell<Peaks>>,
    lines: Vec<String>,
}

struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(8.0)).bg(rgb(0xffffff))
    }
}

impl Render for Example {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let peaks = self.peaks.clone();
        let view = cx.entity();
        let mut content = div().flex().flex_col();
        for row in 0..40 {
            content = content.child(
                div()
                    .h(px(30.0))
                    .px_2()
                    .bg(rgb(if row % 2 == 0 { 0x252a33 } else { 0x2d333d }))
                    .text_color(rgb(0xf0f1f3))
                    .child(format!("row {row}")),
            );
        }
        let wrapper = elastic_scroll(
            "elastic",
            ScrollSource::Handle(self.handle.clone()),
            div()
                .id("scroll")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.handle)
                .child(content),
        )
        .reliable_phases(true)
        .on_change(move |offset, _, cx| {
            let mut peaks = peaks.borrow_mut();
            peaks.current = offset;
            peaks.top = peaks.top.max(offset);
            peaks.bottom = peaks.bottom.min(offset);
            drop(peaks);
            view.update(cx, |_, cx| cx.notify());
        });
        div()
            .size_full()
            .bg(rgb(0x101216))
            .p_4()
            .flex()
            .flex_row()
            .gap_4()
            .child(
                div()
                    .id("drag-source")
                    .w(px(40.0))
                    .h(px(40.0))
                    .bg(rgb(0x405470))
                    .on_drag((), |_, _, _, cx| cx.new(|_| DragGhost)),
            )
            .child(
                div()
                    .w(px(320.0))
                    .h(px(300.0))
                    .bg(rgb(0x181a1f))
                    .child(wrapper),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_color(rgb(0xf0f1f3))
                    .children(self.lines.iter().map(|line| div().child(line.clone()))),
            )
    }
}

fn wheel(window: &mut Window, cx: &mut App, phase: TouchPhase, dy: f32) {
    window.dispatch_event(
        PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: point(px(200.0), px(150.0)),
            delta: ScrollDelta::Pixels(point(px(0.0), px(dy))),
            touch_phase: phase,
            ..Default::default()
        }),
        cx,
    );
}

/// A touch drag of `dy` px, release and three momentum events of `momentum` px.
async fn touch_fling(cx: &mut vespertine::AsyncApp, window: vespertine::AnyWindowHandle, dy: f32) {
    let step = |cx: &mut vespertine::AsyncApp, phase: TouchPhase, delta: f32| {
        cx.update_window(window, |_, window, cx| wheel(window, cx, phase, delta))
            .ok();
    };
    step(cx, TouchPhase::Started, 0.0);
    cx.background_executor()
        .timer(Duration::from_millis(16))
        .await;
    step(cx, TouchPhase::Moved, dy);
    cx.background_executor()
        .timer(Duration::from_millis(16))
        .await;
    step(cx, TouchPhase::Ended, 0.0);
    for _ in 0..6 {
        cx.background_executor()
            .timer(Duration::from_millis(8))
            .await;
        step(cx, TouchPhase::Moved, dy);
    }
}

async fn mouse_wheel(cx: &mut vespertine::AsyncApp, window: vespertine::AnyWindowHandle, dy: f32) {
    for _ in 0..40 {
        cx.background_executor()
            .timer(Duration::from_millis(8))
            .await;
        cx.update_window(window, |_, window, cx| {
            wheel(window, cx, TouchPhase::Moved, dy)
        })
        .ok();
    }
}

fn main() {
    let script = std::env::var_os("ELASTIC_SCRIPT").is_some_and(|value| value == "1");
    application().run(move |cx: &mut App| {
        cx.set_global(ReliableScrollPhases(true));
        let bounds = Bounds::centered(None, size(px(720.0), px(360.0)), cx);
        let peaks = Rc::new(RefCell::new(Peaks::default()));
        let handle = ScrollHandle::new();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                {
                    let peaks = peaks.clone();
                    let handle = handle.clone();
                    move |_, cx| {
                        cx.new(|_| Example {
                            handle,
                            peaks,
                            lines: Vec::new(),
                        })
                    }
                },
            )
            .expect("open the window");
        cx.activate(true);
        let any = window.into();
        cx.spawn(async move |cx| {
            let mut results = Vec::new();
            // Touch fling at the top, then the bottom; a phase-less wheel; then a touch stream
            // during an active drag.
            for (label, bottom) in [("touch fling, top", false), ("touch fling, bottom", true)] {
                *peaks.borrow_mut() = Peaks::default();
                if bottom {
                    handle.set_offset(point(px(0.0), -handle.max_offset().y));
                }
                touch_fling(cx, any, if bottom { -40.0 } else { 40.0 }).await;
                cx.background_executor()
                    .timer(Duration::from_millis(700))
                    .await;
                let p = peaks.borrow();
                results.push((label, p.top.max(-p.bottom)));
            }
            *peaks.borrow_mut() = Peaks::default();
            handle.set_offset(point(px(0.0), px(0.0)));
            mouse_wheel(cx, any, 60.0).await;
            results.push(("phase-less wheel, top", {
                let p = peaks.borrow();
                p.top.max(-p.bottom)
            }));
            *peaks.borrow_mut() = Peaks::default();
            cx.update_window(any, |_, window, cx| {
                let origin = point(px(36.0), px(36.0));
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        position: origin,
                        modifiers: Default::default(),
                        button: MouseButton::Left,
                        click_count: 1,
                        first_mouse: false,
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: origin + point(px(40.0), px(40.0)),
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Default::default(),
                    }),
                    cx,
                );
            })
            .ok();
            cx.background_executor()
                .timer(Duration::from_millis(50))
                .await;
            touch_fling(cx, any, 40.0).await;
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            results.push(("touch during an active drag", {
                let p = peaks.borrow();
                p.top.max(-p.bottom)
            }));
            cx.update_window(any, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        position: point(px(76.0), px(76.0)),
                        button: MouseButton::Left,
                        modifiers: Default::default(),
                        click_count: 1,
                    }),
                    cx,
                );
            })
            .ok();
            let ok = results[0].1 > 5.0
                && results[1].1 > 5.0
                && results[2].1 == 0.0
                && results[3].1 == 0.0;
            for (label, peak) in &results {
                println!("{label}: peak band offset {peak:.1} px");
            }
            println!("elastic example: {}", if ok { "ok" } else { "FAILED" });
            if script {
                std::process::exit(if ok { 0 } else { 1 });
            }
        })
        .detach();
    });
}
