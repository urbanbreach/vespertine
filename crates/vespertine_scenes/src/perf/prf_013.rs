//! PERF-PRF-013: a ticker of 100 numeric cells, every string unique per cell and frame
//! (spec PRF-013, performance workload).

use crate::common::{INK, SANS, WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

const CELLS: u64 = 100;
const COLUMNS: u64 = 5;

pub struct TickerScene;

impl TickerScene {
    pub fn new() -> Self {
        Self
    }
}

/// `value` with comma thousands separators.
fn with_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut text = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            text.push(',');
        }
        text.push(digit);
    }
    text
}

/// Text of cell `i` in frame `f`; only the characters `0123456789.,+-%$KMBT` occur.
fn cell_text(f: u64, i: u64) -> SharedString {
    let n = f * 128 + i;
    let text = match i % 4 {
        0 => {
            let sign = if n % 2 == 0 { '+' } else { '-' };
            format!("{sign}{}.{:02}", n / 100, n % 100)
        }
        1 => format!("{}.{:02}%", n / 100, n % 100),
        2 => {
            let unit = ['K', 'M', 'B', 'T'][(n % 4) as usize];
            format!("${}.{:02}{unit}", n / 1000, (n / 10) % 100)
        }
        _ => format!("+{}", with_thousands(7 * n)),
    };
    text.into()
}

impl Scene for TickerScene {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let mut root = div()
            .relative()
            .size_full()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .line_height(px(18.0))
            .text_color(hex(INK));
        for i in 0..CELLS {
            root = root.child(
                div()
                    .absolute()
                    .left(px(8.0 + 172.0 * (i % COLUMNS) as f32))
                    .top(px(8.0 + 22.0 * (i / COLUMNS) as f32))
                    .w(px(168.0))
                    .h(px(22.0))
                    .whitespace_nowrap()
                    .child(cell_text(f, i)),
            );
        }
        root.into_any_element()
    }
}

register_scene!("PERF-PRF-013", TickerScene::new);
