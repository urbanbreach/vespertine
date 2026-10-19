//! FID-INP-002-1 / FID-INP-002-2: hover state after a wheel scroll under a stationary pointer.
//!
//! The scene drives its own input through the window's ordinary input path on a timeline
//! measured from the first rendered frame: a pointer move to (150, 110) at 50 ms, one precise
//! wheel event of -40 px at 100 ms and, for the suppressed variant, a zero-delta wheel event
//! every 16 ms afterwards.

use std::time::Duration;

use crate::common::{SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, CursorStyle, InputEvent as _, InteractiveElement, IntoElement, Modifiers,
    MouseMoveEvent, ParentElement, ScrollDelta, ScrollWheelEvent, StatefulInteractiveElement,
    Styled, TouchPhase, Window, div, point, px,
};

const POINTER: (f32, f32) = (150.0, 110.0);
const ROWS: usize = 12;

struct HoverAfterWheel {
    /// Keep extending the suppression interval with zero-delta wheel events.
    suppressed: bool,
    started: bool,
}

impl HoverAfterWheel {
    fn settled() -> Self {
        Self {
            suppressed: false,
            started: false,
        }
    }

    fn suppressed() -> Self {
        Self {
            suppressed: true,
            started: false,
        }
    }

    fn start_input(&mut self, window: &mut Window, cx: &mut App) {
        if self.started {
            return;
        }
        self.started = true;
        let suppressed = self.suppressed;
        let position = point(px(POINTER.0), px(POINTER.1));
        window
            .spawn(cx, async move |cx| {
                let executor = cx.background_executor().clone();
                executor.timer(Duration::from_millis(50)).await;
                cx.update(|window, cx| {
                    window.dispatch_event(
                        MouseMoveEvent {
                            position,
                            pressed_button: None,
                            modifiers: Modifiers::default(),
                        }
                        .to_platform_input(),
                        cx,
                    );
                })
                .ok();
                executor.timer(Duration::from_millis(50)).await;
                let wheel = move |delta_y: f32| ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.0), px(delta_y))),
                    modifiers: Modifiers::default(),
                    touch_phase: TouchPhase::Moved,
                };
                cx.update(|window, cx| {
                    window.dispatch_event(wheel(-40.0).to_platform_input(), cx);
                })
                .ok();
                if suppressed {
                    loop {
                        executor.timer(Duration::from_millis(16)).await;
                        cx.update(|window, cx| {
                            window.dispatch_event(wheel(0.0).to_platform_input(), cx);
                        })
                        .ok();
                    }
                }
            })
            .detach();
    }
}

fn row(index: usize) -> impl IntoElement {
    let fill = if index.is_multiple_of(2) { 0xffffff } else { 0xf8f8f8 };
    let row = div()
        .id(("row", index))
        .flex_none()
        .flex()
        .items_center()
        .w(px(460.0))
        .h(px(40.0))
        .pt(px(1.0))
        .pl(px(12.0))
        .border_b(px(1.0))
        .border_color(hex(0xdddddd))
        .bg(hex(fill))
        .hover(|style| style.bg(hex(0xd8e8ff)))
        .text_size(px(14.0))
        .text_color(hex(0x202020))
        .child(format!("Row {}", index + 1));
    if index.is_multiple_of(2) {
        row.cursor(CursorStyle::PointingHand)
    } else {
        row.cursor(CursorStyle::IBeam)
    }
}

impl Scene for HoverAfterWheel {
    fn size(&self) -> (f32, f32) {
        (500.0, 300.0)
    }

    fn frame(&mut self, _f: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        self.start_input(window, cx);
        div()
            .size_full()
            .bg(hex(0xf4f4f4))
            .font_family(SANS)
            .child(
                div()
                    .id("viewport")
                    .absolute()
                    .left(px(20.0))
                    .top(px(20.0))
                    .w(px(460.0))
                    .h(px(240.0))
                    .bg(hex(0xffffff))
                    .overflow_y_scroll()
                    .children((0..ROWS).map(row)),
            )
            .into_any_element()
    }
}

register_scene!("FID-INP-002-1", HoverAfterWheel::settled);
register_scene!("FID-INP-002-2", HoverAfterWheel::suppressed);
