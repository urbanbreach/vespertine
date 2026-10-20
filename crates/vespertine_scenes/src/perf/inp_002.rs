//! PERF-INP-002: stationary-pointer wheel scroll over a virtualized list with hover styling.
//!
//! The pointer rests at (440, 240). Every frame dispatches one precise-pixel wheel event at that
//! position (through the window's ordinary input path, after the frame has been drawn) that moves
//! the list by +-40 logical pixels along a triangle wave of period 376 frames.

use crate::common::{SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, InputEvent as _, InteractiveElement, IntoElement, Modifiers, MouseMoveEvent,
    ParentElement, ScrollDelta, ScrollWheelEvent, Styled, TouchPhase, UniformListScrollHandle,
    Window, div, point, px, uniform_list,
};

const ROWS: usize = 200;
const ROW_HEIGHT: f32 = 40.0;
const PERIOD: u64 = 376;

/// Row offset (in rows) of the triangle wave at frame `f`.
fn triangle(f: u64) -> u64 {
    let phase = f % PERIOD;
    if phase <= PERIOD / 2 {
        phase
    } else {
        PERIOD - phase
    }
}

pub struct WheelHover {
    scroll: UniformListScrollHandle,
    /// Hover suppression stays enabled unless the scene is the comparison variant.
    suppression: bool,
    configured: bool,
}

impl WheelHover {
    pub fn new() -> Self {
        Self {
            scroll: UniformListScrollHandle::new(),
            suppression: true,
            configured: false,
        }
    }

    /// The same workload with hover suppression switched off, for comparison.
    pub fn without_suppression() -> Self {
        Self {
            suppression: false,
            ..Self::new()
        }
    }
}

fn row(ix: usize) -> impl IntoElement {
    let fill = if ix.is_multiple_of(2) {
        0xffffff
    } else {
        0xf8f8f8
    };
    let row = div()
        .id(("row", ix))
        .flex()
        .items_center()
        .w_full()
        .h(px(ROW_HEIGHT))
        .pl(px(12.0))
        .border_b(px(1.0))
        .border_color(hex(0xdddddd))
        .bg(hex(fill))
        .hover(|style| style.bg(hex(0xd8e8ff)))
        .text_size(px(14.0))
        .text_color(hex(0x202020))
        .child(format!("Row {}", ix + 1));
    if ix.is_multiple_of(2) {
        row.cursor_text()
    } else {
        row.cursor_pointer()
    }
}

impl Scene for WheelHover {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        let position = point(px(440.0), px(240.0));
        if !self.configured {
            window.set_hover_suppression_enabled(self.suppression);
            self.configured = true;
        }
        if f == 0 {
            window.defer(cx, move |window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
            });
        } else {
            let rows = triangle(f) as f32 - triangle(f - 1) as f32;
            let delta_y = -rows * ROW_HEIGHT;
            window.defer(cx, move |window, cx| {
                window.dispatch_event(
                    ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(0.0), px(delta_y))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                    }
                    .to_platform_input(),
                    cx,
                );
            });
        }
        div()
            .size_full()
            .bg(hex(0xffffff))
            .font_family(SANS)
            .child(
                uniform_list("rows", ROWS, |range, _window, _cx| {
                    range.map(row).collect::<Vec<_>>()
                })
                .track_scroll(&self.scroll)
                .h_full(),
            )
            .into_any_element()
    }
}

register_scene!("PERF-INP-002", WheelHover::new);
register_scene!(
    "PERF-INP-002-NOSUPPRESSION",
    WheelHover::without_suppression
);
