//! FID-LAY-002: trailing viewport reservation on a five-row list.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, IntoElement, ListAlignment, ListState, ParentElement, Styled, Window, div,
    list, px,
};

const HEIGHTS: [f32; 5] = [40.0, 40.0, 60.0, 50.0, 30.0];

struct Reservation {
    state: ListState,
}

impl Reservation {
    fn new() -> Self {
        let state = ListState::new(HEIGHTS.len(), ListAlignment::Top, px(0.0));
        state.set_trailing_reservation(Some((2, px(20.0))));
        Self { state }
    }
}

impl Scene for Reservation {
    fn size(&self) -> (f32, f32) {
        (420.0, 360.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .size_full()
            .relative()
            .bg(hex(0x202020))
            .child(
                list(self.state.clone(), |ix, _, _| {
                    div()
                        .h(px(HEIGHTS[ix]))
                        .w_full()
                        .pl(px(8.0))
                        .pt(px(8.0))
                        .bg(hex(if ix % 2 == 0 { 0x505050 } else { 0x707070 }))
                        .font_family("Liberation Mono")
                        .text_size(px(14.0))
                        .line_height(px(23.0))
                        .text_color(hex(0xFFFFFF))
                        .child(format!("{ix}"))
                        .into_any_element()
                })
                .absolute()
                .left(px(20.0))
                .top(px(20.0))
                .w(px(380.0))
                .h(px(300.0)),
            )
            .into_any_element()
    }
}

register_scene!("FID-LAY-002", Reservation::new);
