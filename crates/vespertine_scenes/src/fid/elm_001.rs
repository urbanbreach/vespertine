//! FID-ELM-001-1 (windowed list with horizontal padding) and FID-ELM-001-2 (all four paddings).

use crate::common::{hex, SANS};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, IntoElement, ListAlignment, ListOffset, ListState, ParentElement, Styled,
    Window, div, list, px,
};

struct Windowed {
    state: ListState,
}

impl Windowed {
    fn new() -> Self {
        // 100000 logical rows with a 24px estimate; rows 50..90 are mounted.
        let state = ListState::new(40, ListAlignment::Top, px(512.0));
        state.set_estimated_item_height(Some(px(24.0)));
        state.set_logical_item_count(Some(100_000.0));
        state.set_window_start(50);
        state.scroll_to(ListOffset {
            item_ix: 50,
            offset_in_item: px(0.0),
        });
        Self { state }
    }
}

impl Scene for Windowed {
    fn size(&self) -> (f32, f32) {
        (600.0, 420.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .size_full()
            .relative()
            .bg(hex(0x181818))
            .child(
                list(self.state.clone(), |ix, _, _| {
                    div()
                        .h(px(24.0))
                        .w_full()
                        .bg(hex(if ix % 2 == 0 { 0x202020 } else { 0x282828 }))
                        .font_family(SANS)
                        .text_size(px(14.0))
                        .line_height(px(24.0))
                        .text_color(hex(0xEEEEEE))
                        .child(format!("Row {ix}"))
                        .into_any_element()
                })
                .absolute()
                .left(px(20.0))
                .top(px(20.0))
                .w(px(560.0))
                .h(px(360.0))
                .pl(px(12.0))
                .pr(px(12.0))
                .bg(hex(0x202020)),
            )
            .into_any_element()
    }
}

register_scene!("FID-ELM-001-1", Windowed::new);

struct Padded {
    state: ListState,
}

impl Padded {
    fn new() -> Self {
        Self {
            state: ListState::new(3, ListAlignment::Top, px(0.0)),
        }
    }
}

impl Scene for Padded {
    fn size(&self) -> (f32, f32) {
        (400.0, 260.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .size_full()
            .relative()
            .bg(hex(0xFFFFFF))
            .child(
                list(self.state.clone(), |ix, _, _| {
                    div()
                        .h(px(40.0))
                        .w_full()
                        .bg(hex([0xFF0000, 0x00FF00, 0x0000FF][ix]))
                        .into_any_element()
                })
                .absolute()
                .left(px(40.0))
                .top(px(30.0))
                .w(px(260.0))
                .h(px(180.0))
                .pl(px(20.0))
                .pr(px(40.0))
                .pt(px(10.0))
                .pb(px(10.0))
                .bg(hex(0xE0E0E0)),
            )
            .into_any_element()
    }
}

register_scene!("FID-ELM-001-2", Padded::new);
