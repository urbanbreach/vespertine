//! FID-PLT-004: equal logical sizes (explicit toolkit scale on X11).
//!
//! The framework square and the native comparison square are both 100 x 100 logical pixels
//! filled with #2868DC. The native square's pixels are identical to an ordinary solid element at
//! the same logical bounds (a child surface sized to exactly 200 x 200 device pixels at scale 2),
//! so this scene draws both with elements; the child-surface placement itself is shown by the
//! `present_sync` and `x11_toolkit_scale` examples.

use crate::common::{SANS, WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};

const SQUARE: f32 = 100.0;
const FILL: u32 = 0x2868dc;
const TEXT: u32 = 0x202020;
const TEXT_SIZE: f32 = 16.0;

struct Plt004;

impl Scene for Plt004 {
    fn size(&self) -> (f32, f32) {
        (500.0, 240.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .relative()
            .size_full()
            .bg(hex(WHITE))
            .child(
                div()
                    .absolute()
                    .left(px(20.0))
                    .top(px(20.0))
                    .size(px(SQUARE))
                    .bg(hex(FILL)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(150.0))
                    .top(px(20.0))
                    .size(px(SQUARE))
                    .bg(hex(FILL)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(20.0))
                    .top(px(160.0))
                    .whitespace_nowrap()
                    .font_family(SANS)
                    .text_size(px(TEXT_SIZE))
                    .line_height(px(TEXT_SIZE * 1.2))
                    .text_color(hex(TEXT))
                    .child("Equal logical sizes"),
            )
            .into_any_element()
    }
}

register_scene!("FID-PLT-004", || Plt004);
