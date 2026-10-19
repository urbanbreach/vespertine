//! FID-TXT-008: a document selection across three single-line runs, washed from shaped geometry.

use crate::common::{SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, IntoElement, ParentElement, Rgba, Styled, Window, div, px,
};
use vespertine_richtext::{
    RichText, RunKey, SelectionRequest, TextService, selection_color, text_root,
};

const BACKGROUND: u32 = 0x181818;
const INK: u32 = 0xeeeeee;
const LINES: [&str; 3] = ["Select across", "multiple text rows", "without moving glyphs"];

struct SelectAcross;

fn key(row: u32) -> RunKey {
    RunKey::new("fid-txt-008", row)
}

impl Scene for SelectAcross {
    fn size(&self) -> (f32, f32) {
        (600.0, 220.0)
    }

    fn frame(&mut self, _f: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        // All of the first two runs and the first word of the third.
        let service = TextService::of(window, cx);
        if !service.selection_has_text() {
            service.select_runs(SelectionRequest {
                from: (key(0), 0),
                to: (key(2), "without".len()),
            });
        }
        let region = div()
            .absolute()
            .left(px(20.))
            .top(px(20.))
            .w(px(560.))
            .h(px(160.))
            .font_family(SANS)
            .text_size(px(16.))
            .line_height(px(24.))
            .text_color(hex(INK))
            .children(LINES.iter().enumerate().map(|(row, line)| {
                RichText::new(("line", row), *line).key(key(row as u32))
            }));
        text_root(
            div()
                .size_full()
                .relative()
                .bg(hex(BACKGROUND))
                .child(selection_color(Rgba { r: 0x44 as f32 / 255., g: 0x66 as f32 / 255., b: 0xcc as f32 / 255., a: 0x80 as f32 / 255. }, region)),
        )
        .into_any_element()
    }
}

register_scene!("FID-TXT-008", || SelectAcross);
