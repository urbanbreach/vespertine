//! PERF-TXT-005: scrolling subpixel-positioned text rows (spec TXT-005, performance workload).

use crate::common::{INK, SANS, WHITE, hex};
use crate::corpus;
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, InteractiveElement, IntoElement, ParentElement, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, div, point, px,
};

const WIDTH: f32 = 880.0;
const HEIGHT: f32 = 480.0;
const ROWS: usize = 200;
const ROW_HEIGHT: f32 = 22.0;
const CHARS_PER_ROW: usize = 40;
/// Content height in device pixels (scale factor 2): the scroll offset wraps every this many frames.
const SCROLL_PERIOD: u64 = 8_800;

pub struct ScrollTextScene {
    rows: Vec<SharedString>,
    scroll: ScrollHandle,
}

impl ScrollTextScene {
    pub fn new() -> Self {
        let rows = (0..ROWS)
            .map(|k| {
                let mut text = corpus::line(k);
                text.truncate(CHARS_PER_ROW);
                SharedString::from(text)
            })
            .collect();
        Self {
            rows,
            scroll: ScrollHandle::new(),
        }
    }
}

impl Scene for ScrollTextScene {
    fn size(&self) -> (f32, f32) {
        (WIDTH, HEIGHT)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        // One device pixel per frame at scale factor 2.
        let offset = -((f % SCROLL_PERIOD) as f32 / 2.0);
        self.scroll.set_offset(point(px(0.0), px(offset)));

        let mut content = div()
            .relative()
            .w(px(WIDTH))
            .h(px(ROWS as f32 * ROW_HEIGHT))
            .font_family(SANS)
            .text_size(px(14.0))
            .line_height(px(ROW_HEIGHT))
            .text_color(hex(INK));
        for (k, text) in self.rows.iter().enumerate() {
            content = content.child(
                div()
                    .absolute()
                    .left(px(8.0 + 0.125 * (k % 8) as f32))
                    .top(px(ROW_HEIGHT * k as f32))
                    .h(px(ROW_HEIGHT))
                    .whitespace_nowrap()
                    .child(text.clone()),
            );
        }
        div()
            .id("txt005-scroll")
            .size_full()
            .bg(hex(WHITE))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(content)
            .into_any_element()
    }
}

register_scene!("PERF-TXT-005", ScrollTextScene::new);
