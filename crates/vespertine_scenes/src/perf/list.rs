//! PERF-LIST: virtualized list scroll.

use crate::scene::{Scene, register_scene};
use crate::common::{INK, SANS, SURFACE, WHITE, hex, hsl_deg};
use vespertine::{
    AnyElement, App, Div, IntoElement, ParentElement, Styled,
    UniformListScrollHandle, Window, div, point, px, uniform_list,
};

const ROWS: usize = 100_000;
const ROW_HEIGHT: f32 = 24.0;
const VIEWPORT_HEIGHT: f32 = 500.0;

pub struct ListScene {
    scroll: UniformListScrollHandle,
}

impl ListScene {
    pub fn new() -> Self {
        Self {
            scroll: UniformListScrollHandle::new(),
        }
    }
}

fn row(i: usize) -> Div {
    let mut row = div()
        .h(px(ROW_HEIGHT))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .pl(px(12.0))
        .gap(px(8.0));
    if i % 2 == 1 {
        row = row.bg(hex(SURFACE));
    }
    row.child(
        div()
            .flex_none()
            .w(px(12.0))
            .h(px(12.0))
            .rounded(px(3.0))
            .bg(hsl_deg(((i * 37) % 360) as u32, 0.6, 0.55)),
    )
    .child(
        div()
            .whitespace_nowrap()
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(format!("Row {i}: lorem ipsum dolor sit amet")),
    )
}

impl Scene for ListScene {
    fn size(&self) -> (f32, f32) {
        (800.0, 500.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let max_scroll = ROWS as u64 * ROW_HEIGHT as u64 - VIEWPORT_HEIGHT as u64;
        let offset = (f * 7) % max_scroll;
        self.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.0), px(-(offset as f32))));
        div()
            .size_full()
            .bg(hex(WHITE))
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

register_scene!("PERF-LIST", ListScene::new);
