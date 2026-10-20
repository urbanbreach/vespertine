//! FID-ELM-008: layout-neutral element outlines.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, IntoElement, Outline, ParentElement, Styled, Window, div, px};

struct Elm008;

impl Scene for Elm008 {
    fn size(&self) -> (f32, f32) {
        (400.0, 200.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let rectangle = |left: f32| {
            div()
                .absolute()
                .left(px(left))
                .top(px(50.))
                .w(px(120.))
                .h(px(80.))
                .bg(hex(0x202020))
        };
        div()
            .size_full()
            .relative()
            .bg(hex(0xFFFFFF))
            .child(rectangle(40.).rounded(px(8.)).outline(Outline::new(
                px(2.),
                hex(0x0066FF),
                px(3.),
            )))
            .child(
                rectangle(220.)
                    .rounded_tr(px(8.))
                    .rounded_br(px(8.))
                    .rounded_bl(px(8.))
                    .outline(Outline::new(px(3.), hex(0xFF8000), px(-5.))),
            )
            .into_any_element()
    }
}

register_scene!("FID-ELM-008", || Elm008);
