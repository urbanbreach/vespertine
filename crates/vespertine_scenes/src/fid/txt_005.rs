//! FID-TXT-005: run-level glyph painting and translation-stable origins (spec TXT-005).
//!
//! Clips A/B and C/D must be pixel identical (B and D are scroll containers whose offset brings
//! the text to the position of A and C); clip E shows only the lower part of a tall line box.

use crate::common::{WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Div, InteractiveElement, IntoElement, ParentElement, ScrollHandle,
    StatefulInteractiveElement, Styled, Window, div, point, px,
};

const SAMPLE: &str = "Agjp 012345";
const CLIP_WIDTH: f32 = 260.0;
const CLIP_HEIGHT: f32 = 100.0;

pub struct ClipScene {
    scroll_b: ScrollHandle,
    scroll_d: ScrollHandle,
}

impl ClipScene {
    pub fn new() -> Self {
        Self {
            scroll_b: ScrollHandle::new(),
            scroll_d: ScrollHandle::new(),
        }
    }
}

/// The sample line (24 px, #202020) at `(x, y)` with the given line height.
fn sample(x: f32, y: f32, line_height: f32) -> impl IntoElement {
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .whitespace_nowrap()
        .text_size(px(24.0))
        .line_height(px(line_height))
        .text_color(hex(0x202020))
        .child(SAMPLE)
}

/// A clipping box of the given size at `(x, y)`.
fn clip(x: f32, y: f32, height: f32) -> Div {
    div()
        .absolute()
        .left(px(x))
        .top(px(y))
        .w(px(CLIP_WIDTH))
        .h(px(height))
        .overflow_hidden()
}

impl Scene for ClipScene {
    fn size(&self) -> (f32, f32) {
        (620.0, 360.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        self.scroll_b.set_offset(point(px(0.0), px(-1000.0)));
        self.scroll_d.set_offset(point(px(-1000.0), px(-1000.0)));

        div()
            .relative()
            .size_full()
            .bg(hex(WHITE))
            .font_family("Noto Sans")
            .child(clip(20.0, 20.0, CLIP_HEIGHT).child(sample(12.0, 12.0, 32.0)))
            .child(
                div()
                    .id("clip-b")
                    .absolute()
                    .left(px(320.0))
                    .top(px(20.0))
                    .w(px(CLIP_WIDTH))
                    .h(px(CLIP_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_b)
                    .child(
                        div()
                            .relative()
                            .w(px(CLIP_WIDTH))
                            .h(px(2400.0))
                            .child(sample(12.0, 1012.0, 32.0)),
                    ),
            )
            .child(clip(20.0, 140.0, CLIP_HEIGHT).child(sample(12.125, 12.25, 32.0)))
            .child(
                div()
                    .id("clip-d")
                    .absolute()
                    .left(px(320.0))
                    .top(px(140.0))
                    .w(px(CLIP_WIDTH))
                    .h(px(CLIP_HEIGHT))
                    .overflow_scroll()
                    .track_scroll(&self.scroll_d)
                    .child(
                        div()
                            .relative()
                            .w(px(2400.0))
                            .h(px(2400.0))
                            .child(sample(1012.125, 1012.25, 32.0)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .left(px(20.0))
                    .top(px(250.0))
                    .whitespace_nowrap()
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .text_color(hex(0x303030))
                    .child("The two clipped samples must match"),
            )
            .child(clip(20.0, 290.0, 40.0).child(sample(12.0, -60.0, 120.0)))
            .into_any_element()
    }
}

register_scene!("FID-TXT-005", ClipScene::new);
