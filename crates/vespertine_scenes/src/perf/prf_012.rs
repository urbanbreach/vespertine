//! PERF-PRF-012: a scrolling column of labels over a growing vocabulary, with a font addition
//! halfway through phase B (spec PRF-012, performance workload).
//!
//! Frames 0..360 are phase A (vocabulary of 150 labels), frames 360.. are phase B (2,000 labels).
//! The two measurement configurations of the spec (run A: warm-up 120 / 240 frames, run B:
//! warm-up 360 / 360 frames) are selected with the runner's `--warmup` and `--frames`.

use std::borrow::Cow;

use crate::common::{INK, SANS, WHITE, hex};
use crate::corpus;
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, IntoElement, ParentElement, SharedString, Styled, Window, div, px,
};

const ROWS: u64 = 22;
const PHASE_B_START: u64 = 360;
const VOCABULARY_A: u64 = 150;
const VOCABULARY_B: u64 = 2_000;
/// The frame in which the extra font is registered.
const FONT_ADDED_AT: u64 = 540;
const EXTRA_FONT_PATH: &str = "/usr/share/fonts/liberation/LiberationSerif-Regular.ttf";

/// The 64-word list `W` of the PERF-TEXT corpus, indexed by word number.
///
/// `corpus` exposes the list only through corpus lines, where word `w` of line `k` is
/// `W[(7k + 13w) mod 64]`; with `w = 0` and `k = 55 i mod 64` (55 is the inverse of 7 modulo 64)
/// the first word of line `k` is `W[i]`.
pub(super) fn words() -> Vec<&'static str> {
    (0..64)
        .map(|i| corpus::line_words((i * 55) % 64)[0])
        .collect()
}

pub struct LabelScene {
    words: Vec<&'static str>,
    /// Bytes of the extra font until it has been registered.
    extra_font: Option<Vec<u8>>,
}

impl LabelScene {
    pub fn new() -> Self {
        // Read here so that the file access is not part of any measured frame.
        let extra_font = match std::fs::read(EXTRA_FONT_PATH) {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                eprintln!("PERF-PRF-012: cannot read {EXTRA_FONT_PATH}: {error}");
                None
            }
        };
        Self {
            words: words(),
            extra_font,
        }
    }

    /// Label string `L(j)`.
    fn label(&self, j: u64) -> SharedString {
        let word = |index: u64| self.words[(index % 64) as usize];
        format!(
            "{} {} {} #{j}",
            word(j * 7),
            word(j * 11 + 3),
            word(j * 13 + 5)
        )
        .into()
    }
}

impl Scene for LabelScene {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, cx: &mut App) -> AnyElement {
        if f >= FONT_ADDED_AT {
            if let Some(bytes) = self.extra_font.take() {
                if let Err(error) = cx.text_system().add_fonts(vec![Cow::Owned(bytes)]) {
                    eprintln!("PERF-PRF-012: adding {EXTRA_FONT_PATH}: {error}");
                }
            }
        }

        let vocabulary = if f < PHASE_B_START {
            VOCABULARY_A
        } else {
            VOCABULARY_B
        };
        let scroll = 3 * f;
        let mut column = div()
            .absolute()
            .left(px(8.0))
            .top(px(8.0))
            .flex()
            .flex_col()
            .font_family(SANS)
            .text_size(px(14.0))
            .line_height(px(18.0))
            .text_color(hex(INK));
        for i in 0..ROWS {
            column = column.child(
                div()
                    .flex_none()
                    .w(px(864.0))
                    .h(px(20.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.label((scroll + i) % vocabulary)),
            );
        }
        div()
            .relative()
            .size_full()
            .bg(hex(WHITE))
            .child(column)
            .into_any_element()
    }
}

register_scene!("PERF-PRF-012", LabelScene::new);
