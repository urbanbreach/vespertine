//! FID-TXT-006: shaped rich text (mixed weights/styles, decorations, tabular numerals).

use crate::common::{SANS, hex};
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{
    AnyElement, App, FontFeatures, FontStyle, FontWeight, Hsla, IntoElement, ParentElement,
    StrikethroughStyle, Styled, StyledText, TextRun, UnderlineStyle, Window, div, font, px,
};

const BACKGROUND: u32 = 0x202020;
const INK: u32 = 0xeeeeee;
const MARK: u32 = 0x3a5a8c;

#[derive(Clone, Copy)]
struct Run {
    len: usize,
    weight: FontWeight,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    background: Option<u32>,
    tabular: bool,
}

impl Run {
    const fn plain(len: usize) -> Self {
        Run {
            len,
            weight: FontWeight::NORMAL,
            italic: false,
            underline: false,
            strikethrough: false,
            background: None,
            tabular: false,
        }
    }
}

fn text_run(run: Run) -> TextRun {
    let color: Hsla = hex(INK);
    let mut run_font = font(SANS);
    run_font.weight = run.weight;
    if run.italic {
        run_font.style = FontStyle::Italic;
    }
    if run.tabular {
        run_font.features = FontFeatures(Arc::new(vec![("tnum".into(), 1)]));
    }
    TextRun {
        len: run.len,
        font: run_font,
        color,
        background_color: run.background.map(hex),
        underline: run.underline.then_some(UnderlineStyle {
            thickness: px(1.),
            color: Some(color),
            wavy: false,
        }),
        strikethrough: run.strikethrough.then_some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(color),
        }),
    }
}

fn block(top: f32, size: f32, line_height: f32, text: &'static str, runs: &[Run]) -> AnyElement {
    debug_assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
    div()
        .absolute()
        .left(px(20.))
        .top(px(top))
        .w(px(350.))
        .font_family(SANS)
        .text_size(px(size))
        .line_height(px(line_height))
        .text_color(hex(INK))
        .child(StyledText::new(text).with_runs(runs.iter().copied().map(text_run).collect()))
        .into_any_element()
}

struct RichText;

impl Scene for RichText {
    fn size(&self) -> (f32, f32) {
        (480.0, 220.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let bold = FontWeight::BOLD;
        let tabular = Run {
            tabular: true,
            ..Run::plain(11)
        };
        div()
            .size_full()
            .relative()
            .bg(hex(BACKGROUND))
            .child(block(
                20.,
                14.,
                20.,
                "Plain bold italic underlined",
                &[
                    Run::plain(6),
                    Run {
                        weight: bold,
                        ..Run::plain(5)
                    },
                    Run {
                        italic: true,
                        ..Run::plain(7)
                    },
                    Run {
                        underline: true,
                        ..Run::plain(10)
                    },
                ],
            ))
            .child(block(
                60.,
                14.,
                20.,
                "struck marked",
                &[
                    Run {
                        strikethrough: true,
                        ..Run::plain(6)
                    },
                    Run::plain(1),
                    Run {
                        background: Some(MARK),
                        ..Run::plain(6)
                    },
                ],
            ))
            .child(block(110., 20., 26., "001 010 100", &[tabular]))
            .child(block(160., 20., 26., "111 000 888", &[tabular]))
            .into_any_element()
    }
}

register_scene!("FID-TXT-006", || RichText);
