//! FID-LAY-005: a deferred floating layer attached to the bottom-right corner of its trigger.

use crate::common::{WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, FloatingAlign, FloatingSide, IntoElement, ParentElement, Styled, Window, div,
    floating_layer, px,
};

struct Lay005;

impl Scene for Lay005 {
    fn size(&self) -> (f32, f32) {
        (320.0, 200.0)
    }

    fn frame(&mut self, _frame: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .size_full()
            .bg(hex(WHITE))
            .child(
                div()
                    .absolute()
                    .left(px(100.))
                    .top(px(60.))
                    .w(px(80.))
                    .h(px(30.))
                    .bg(hex(0x808080))
                    .child(
                        floating_layer("fid-lay-005-layer")
                            .side(FloatingSide::Bottom)
                            .align(FloatingAlign::End)
                            .gap(px(6.))
                            .snap_margin(px(8.))
                            .priority(1)
                            .w(px(100.))
                            .h(px(50.))
                            .bg(hex(0x1A1A1A)),
                    ),
            )
            .into_any_element()
    }
}

register_scene!("FID-LAY-005", || Lay005);
