//! PERF-DEV-003: the quote-table workload of DEV-003, as a scene.
//!
//! A 200-row quote table below a 28 px header. The per-frame update is a pure function of the
//! frame index: frames 0..400 update 4 rows each (2%), frames 400..800 update 100 rows each
//! (50%), and from frame 800 the table is re-sorted every 15 frames while exactly one row updates
//! on frames with `f mod 15 = 7`. The measurement spans of the specification are runner
//! arguments: sparse `--warmup 30 --frames 200`, dense `--warmup 430 --frames 200`, sorted
//! `--warmup 830 --frames 200`.

use crate::common::{INK, SANS, SURFACE, WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, Div, FontWeight, IntoElement, ParentElement, Styled, Window, div, px,
};

const ROWS: usize = 200;
const HEADER_HEIGHT: f32 = 28.0;
const ROW_HEIGHT: f32 = 26.0;
const COLUMNS: [f32; 4] = [12.0, 160.0, 300.0, 440.0];
const POSITIVE: u32 = 0x1a7f37;
const NEGATIVE: u32 = 0xcf222e;
const SORT_PERIOD: u64 = 15;

#[derive(Clone, Copy)]
struct Quote {
    price: f64,
    previous: f64,
    volume: u64,
}

fn price_at(row: usize, version: u64) -> f64 {
    10.0 + ((37 * row as u64 + 101 * version) % 9000) as f64 / 100.0
}

fn initial(row: usize) -> Quote {
    let price = price_at(row, 0);
    Quote {
        price,
        previous: price,
        volume: 1000 * (row as u64 + 1),
    }
}

/// Table state after the updates of frame `f`; replayed from frame 0 whenever a frame earlier
/// than the last one is asked for.
pub struct Dev003Scene {
    applied: Option<u64>,
    quotes: Vec<Quote>,
    order: Vec<usize>,
}

impl Dev003Scene {
    pub fn new() -> Self {
        Self {
            applied: None,
            quotes: (0..ROWS).map(initial).collect(),
            order: (0..ROWS).collect(),
        }
    }

    fn update_row(&mut self, row: usize, f: u64) {
        let quote = &mut self.quotes[row];
        quote.previous = quote.price;
        quote.price = price_at(row, f);
        quote.volume = 1000 * (row as u64 + 1) + 17 * f;
    }

    fn apply(&mut self, f: u64) {
        let rows = ROWS as u64;
        if f < 400 {
            for j in 0..4 {
                self.update_row(((3 * f + 50 * j) % rows) as usize, f);
            }
        } else if f < 800 {
            for row in (0..ROWS).filter(|row| (*row as u64 + f).is_multiple_of(2)) {
                self.update_row(row, f);
            }
        } else {
            if f % SORT_PERIOD == 7 {
                self.update_row(((f / SORT_PERIOD) % rows) as usize, f);
            }
            if f.is_multiple_of(SORT_PERIOD) {
                let quotes = &self.quotes;
                match (f / SORT_PERIOD) % 3 {
                    0 => self.order.sort_by_key(|row| *row),
                    1 => self
                        .order
                        .sort_by(|a, b| quotes[*b].price.total_cmp(&quotes[*a].price).then(a.cmp(b))),
                    _ => self
                        .order
                        .sort_by(|a, b| quotes[*b].volume.cmp(&quotes[*a].volume).then(a.cmp(b))),
                }
            }
        }
        self.applied = Some(f);
    }

    fn advance_to(&mut self, f: u64) {
        match self.applied {
            Some(done) if done == f => {}
            Some(done) if done < f => {
                for next in done + 1..=f {
                    self.apply(next);
                }
            }
            _ => {
                *self = Self::new();
                for next in 0..=f {
                    self.apply(next);
                }
            }
        }
    }
}

fn volume(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn cell(ix: usize) -> Div {
    let width = COLUMNS.get(ix + 1).map_or(160.0, |next| next - COLUMNS[ix]);
    div().w(px(width)).flex_none()
}

impl Scene for Dev003Scene {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        self.advance_to(f);
        let mut header = div()
            .h(px(HEADER_HEIGHT))
            .flex()
            .flex_none()
            .items_center()
            .pl(px(COLUMNS[0]))
            .font_weight(FontWeight::BOLD);
        for (ix, label) in ["Symbol", "Price", "Change", "Volume"].into_iter().enumerate() {
            header = header.child(cell(ix).child(label));
        }
        let mut table = div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(header);
        for (position, row) in self.order.iter().copied().enumerate() {
            let quote = self.quotes[row];
            let change = (quote.price - quote.previous) / quote.previous * 100.0;
            let mut line = div().h(px(ROW_HEIGHT)).flex().flex_none().items_center().pl(px(COLUMNS[0]));
            if position % 2 == 1 {
                line = line.bg(hex(SURFACE));
            }
            table = table.child(
                line.child(cell(0).child(format!("SYM{row:03}")))
                    .child(cell(1).child(format!("{:.2}", quote.price)))
                    .child(
                        cell(2)
                            .text_color(hex(if change >= 0.0 { POSITIVE } else { NEGATIVE }))
                            .child(format!("{}{:.2}%", if change >= 0.0 { "+" } else { "-" }, change.abs())),
                    )
                    .child(cell(3).child(volume(quote.volume))),
            );
        }
        table.into_any_element()
    }
}

register_scene!("PERF-DEV-003", Dev003Scene::new);
