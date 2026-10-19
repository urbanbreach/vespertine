//! PERF-PRF-011: a 20 x 8 table of tabular-number cells (spec PRF-011, performance workload).

use std::sync::Arc;

use crate::common::{INK, SANS, WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, FontFeatures, IntoElement, ParentElement, SharedString, Styled, Window, div,
    px,
};

const ROWS: usize = 20;
const COLUMNS: usize = 8;
const CELLS: u64 = (ROWS * COLUMNS) as u64;
const LINK: u32 = 0x0550ae;

pub struct TableScene;

impl TableScene {
    pub fn new() -> Self {
        Self
    }
}

/// Table width: alternates 860 / 780 during the first 120 frames, then stays 860.
fn table_width(f: u64) -> f32 {
    if f < 120 && f % 2 == 1 { 780.0 } else { 860.0 }
}

/// Text of cell `k` in frame `f`: `floor(n / 100) "." (n mod 100)`, with a leading "1" for the
/// digit-growth cell of frames that are a multiple of 50.
fn cell_text(f: u64, k: u64) -> SharedString {
    let n = 1_000_000 + ((f * 104_729 + k * 7_919 + 12_345) % 8_999_999);
    let grown = f > 0 && f.is_multiple_of(50) && k == ((f / 50) * 37) % CELLS;
    let prefix = if grown { "1" } else { "" };
    format!("{prefix}{}.{:02}", n / 100, n % 100).into()
}

impl Scene for TableScene {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let decorated = (f / 75) % 2 == 1;
        let color = if decorated { hex(LINK) } else { hex(INK) };
        let tabular = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));

        let mut table = div()
            .absolute()
            .left(px(8.0))
            .top(px(8.0))
            .w(px(table_width(f)))
            .flex()
            .flex_col()
            .font_family(SANS)
            .text_size(px(14.0))
            .line_height(px(18.0))
            .font_features(tabular)
            .text_color(color);
        for row in 0..ROWS as u64 {
            let mut line = div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .h(px(22.0))
                .gap(px(8.0));
            for column in 0..COLUMNS as u64 {
                let mut cell = div()
                    .flex_none()
                    .px(px(6.0))
                    .child(cell_text(f, row * COLUMNS as u64 + column));
                if decorated {
                    cell = cell.underline();
                }
                line = line.child(cell);
            }
            table = table.child(line);
        }
        div()
            .relative()
            .size_full()
            .bg(hex(WHITE))
            .child(table)
            .into_any_element()
    }
}

register_scene!("PERF-PRF-011", TableScene::new);
