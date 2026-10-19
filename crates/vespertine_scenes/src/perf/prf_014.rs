//! PERF-PRF-014: 100 unique labels per frame at changing font sizes (spec PRF-014, performance
//! workload).
//!
//! Configuration 1 is frames 0..720 (sizes 16 / 24 / 16.0001 in blocks of ten frames), configuration
//! 2 is frames 720..1920 (1,100 distinct sizes, then 23). The runner's `--warmup` and `--frames`
//! select the configuration.

use crate::common::{INK, SANS, WHITE, hex};
use crate::scene::{Scene, register_scene};
use super::prf_012::words;
use vespertine::{
    AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

const LABELS: u64 = 100;
const COLUMNS: u64 = 5;
const CONFIG_2_START: u64 = 720;
const SWEEP_END: u64 = 1_820;

pub struct SizeScene {
    words: Vec<&'static str>,
}

impl SizeScene {
    pub fn new() -> Self {
        Self { words: words() }
    }

    /// Label `i` of frame `f`.
    fn label(&self, f: u64, i: u64) -> SharedString {
        let word = |index: u64| self.words[(index % 64) as usize];
        format!(
            "{} {} office affine {}",
            word(i * 7 + f),
            word(i * 3 + f * 5),
            f * 100 + i
        )
        .into()
    }
}

/// Font size (logical px) of frame `f`.
fn font_size(f: u64) -> f32 {
    if f < CONFIG_2_START {
        match (f / 10) % 3 {
            0 => 16.0,
            1 => 24.0,
            _ => 16.0001,
        }
    } else if f < SWEEP_END {
        (12.0 + 0.01 * (f - CONFIG_2_START) as f64) as f32
    } else {
        23.0
    }
}

impl Scene for SizeScene {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let size = px(font_size(f));
        let mut root = div()
            .relative()
            .size_full()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(size)
            .line_height(px(18.0))
            .text_color(hex(INK));
        for i in 0..LABELS {
            root = root.child(
                div()
                    .absolute()
                    .left(px(8.0 + 172.0 * (i % COLUMNS) as f32))
                    .top(px(8.0 + 22.0 * (i / COLUMNS) as f32))
                    .w(px(168.0))
                    .h(px(20.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.label(f, i)),
            );
        }
        root.into_any_element()
    }
}

register_scene!("PERF-PRF-014", SizeScene::new);
