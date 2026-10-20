//! FID-PRF-004: a scrolled list whose content was scrolled on consecutive frames, so that its
//! tiles are composited (spec PRF-004).
//!
//! The scene is its own per-frame driver: it counts the frames it renders, starting at 0, and
//! queues one precise-pixel wheel event of -0.9 logical px after each of the frames 1 through 45
//! (the pointer at (420, 220), inside the viewport), through the window's ordinary input path.
//! The final offset is 40.5 logical px, 81 device px at scale factor 2: an odd translation.
//! The scroll content is the root of a view of its own, so that the runner's per-frame
//! animation request (an animation of the outer view) does not drop the layer.

use crate::common::{SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, AppContext as _, Context, Entity, InputEvent as _, InteractiveElement,
    IntoElement, Modifiers, ParentElement, PathBuilder, Render, ScrollDelta, ScrollWheelEvent,
    StatefulInteractiveElement, Styled, TouchPhase, Window, canvas, div, point, px,
};

const ROWS: usize = 40;
const ROW_HEIGHT: f32 = 40.0;
const VIEWPORT: (f32, f32) = (800.0, 400.0);
/// The frames after which a wheel event is queued, and its delta.
const WHEEL_FRAMES: std::ops::RangeInclusive<u64> = 1..=45;
const WHEEL_DELTA: f32 = -0.9;
const POINTER: (f32, f32) = (420.0, 220.0);
/// The polyline of every row, relative to the top-left corner of the row.
const POLYLINE: [(f32, f32); 4] = [(300.0, 20.0), (320.0, 10.0), (340.0, 25.0), (360.0, 15.0)];

struct Viewport;

fn row(index: usize) -> impl IntoElement {
    let fill = if index.is_multiple_of(2) {
        0xffffff
    } else {
        0xf7f7f7
    };
    div()
        .id(("row", index))
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .w(px(VIEWPORT.0))
        .h(px(ROW_HEIGHT))
        .pl(px(12.0))
        .bg(hex(fill))
        .text_size(px(14.0))
        .line_height(px(20.0))
        .text_color(hex(0x202020))
        .child(format!("Item {}", index + 1))
        .child(
            canvas(
                |_, _, _| {},
                |bounds, _, window, _| {
                    let mut stroke = PathBuilder::stroke(px(1.0));
                    for (step, (x, y)) in POLYLINE.into_iter().enumerate() {
                        let vertex = point(bounds.origin.x + px(x), bounds.origin.y + px(y));
                        if step == 0 {
                            stroke.move_to(vertex);
                        } else {
                            stroke.line_to(vertex);
                        }
                    }
                    if let Ok(path) = stroke.build() {
                        window.paint_path(path, hex(0x2868dc));
                    }
                },
            )
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .size_full(),
        )
}

impl Render for Viewport {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("viewport")
            .size_full()
            .bg(hex(0xffffff))
            .overflow_y_scroll()
            .children((0..ROWS).map(row))
    }
}

pub struct ScrolledList {
    rendered: u64,
    viewport: Option<Entity<Viewport>>,
}

impl ScrolledList {
    pub fn new() -> Self {
        Self {
            rendered: 0,
            viewport: None,
        }
    }
}

impl Scene for ScrolledList {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, _f: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        let rendered = self.rendered;
        self.rendered += 1;
        if WHEEL_FRAMES.contains(&rendered) {
            window.defer(cx, |window, cx| {
                window.dispatch_event(
                    ScrollWheelEvent {
                        position: point(px(POINTER.0), px(POINTER.1)),
                        delta: ScrollDelta::Pixels(point(px(0.0), px(WHEEL_DELTA))),
                        modifiers: Modifiers::default(),
                        touch_phase: TouchPhase::Moved,
                    }
                    .to_platform_input(),
                    cx,
                );
            });
        }
        let viewport = self
            .viewport
            .get_or_insert_with(|| cx.new(|_| Viewport))
            .clone();
        div()
            .size_full()
            .relative()
            .bg(hex(0xeeeeee))
            .font_family(SANS)
            .child(
                div()
                    .absolute()
                    .left(px(20.0))
                    .top(px(20.0))
                    .w(px(VIEWPORT.0))
                    .h(px(VIEWPORT.1))
                    .child(viewport),
            )
            .into_any_element()
    }
}

register_scene!("FID-PRF-004", ScrolledList::new);
