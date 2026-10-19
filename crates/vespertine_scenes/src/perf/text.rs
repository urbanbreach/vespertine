//! PERF-TEXT: dense text (editor-like scrolling).

use std::ops::Range;

use crate::scene::{Scene, register_scene};
use crate::common::{INK, MONO, WHITE, hex};
use crate::corpus;
use vespertine::{
    AnyElement, App, HighlightStyle, Hsla, IntoElement, ParentElement, SharedString, StyledText,
    Styled, Window, div, px,
};

const VISIBLE_LINES: usize = 36;
const WORD_COLORS: [u32; 5] = [0x1f2328, 0xcf222e, 0x0550ae, 0x116329, 0x8250df];

struct Line {
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
}

pub struct TextScene {
    lines: Vec<Line>,
}

impl TextScene {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    fn line(&mut self, k: usize) -> &Line {
        while self.lines.len() <= k {
            let next = self.lines.len();
            self.lines.push(build_line(next));
        }
        &self.lines[k]
    }
}

/// Splits corpus line `k` into one colored run per word (spaces keep the inherited color).
fn build_line(k: usize) -> Line {
    let text = corpus::line(k);
    let mut highlights = Vec::new();
    let mut start = 0;
    for (w, word) in corpus::line_words(k).into_iter().enumerate() {
        if w > 0 {
            start += 1;
        }
        let end = (start + word.len()).min(text.len());
        if start >= end {
            break;
        }
        let color: Hsla = hex(WORD_COLORS[w % WORD_COLORS.len()]);
        highlights.push((
            start..end,
            HighlightStyle {
                color: Some(color),
                ..Default::default()
            },
        ));
        start = end;
    }
    Line {
        text: text.into(),
        highlights,
    }
}

impl Scene for TextScene {
    fn size(&self) -> (f32, f32) {
        (800.0, 500.0)
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let first = (f / 2) as usize;
        let mut root = div()
            .size_full()
            .bg(hex(WHITE))
            .flex()
            .flex_col()
            .font_family(MONO)
            .text_size(px(11.0))
            .line_height(px(13.0))
            .text_color(hex(INK));
        for k in first..first + VISIBLE_LINES {
            let line = self.line(k);
            let text = StyledText::new(line.text.clone()).with_highlights(line.highlights.clone());
            root = root.child(
                div()
                    .flex_none()
                    .h(px(13.0))
                    .pl(px(4.0))
                    .whitespace_nowrap()
                    .child(text),
            );
        }
        root.into_any_element()
    }
}

register_scene!("PERF-TEXT", TextScene::new);
