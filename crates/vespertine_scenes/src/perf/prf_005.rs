//! PERF-PRF-005 (uniform-height rows) and PERF-PRF-005-2 (variable-height rows): virtual lists
//! of 160 rows, each reading its own edit-counter model (spec PRF-005, performance workload).
//!
//! The pointer rests at (440, 240). Frames 0..29 are warm; frames 30..179 dispatch one wheel
//! event of -40 px per frame, frames 180..329 one of +40 px; frames 330..389 increment the edit
//! counter of row `(f - 330) mod 160` and notify its model; frames 390..689 set the scroll offset
//! the way a scrollbar does, along a triangle wave between 0 and 6,000 px.

use crate::common::{BORDER, INK, SANS, SURFACE, WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, AppContext as _, Entity, InputEvent as _, InteractiveElement, IntoElement,
    ListAlignment, ListState, Modifiers, MouseMoveEvent, ParentElement, ScrollDelta,
    ScrollWheelEvent, Styled, TouchPhase, UniformListScrollHandle, Window, div, list, point, px,
    uniform_list,
};

const ROWS: usize = 160;
const SEPARATOR: u32 = 0xeaeef2;
const CODE_HOVER: u32 = 0xeaeef2;
const MONO: &str = "Liberation Mono";
const WARM: u64 = 30;
const WHEEL_DOWN_END: u64 = 180;
const WHEEL_UP_END: u64 = 330;
const UPDATES_END: u64 = 390;
const SWEEP_END: u64 = 690;
const OVERDRAW: f32 = 240.0;

/// The model a row reads: how many times the row was edited.
struct Edits(u32);

pub struct RowList {
    variable: bool,
    models: Vec<Entity<Edits>>,
    scroll: UniformListScrollHandle,
    state: ListState,
}

impl RowList {
    pub fn uniform() -> Self {
        Self::new(false)
    }

    pub fn variable() -> Self {
        Self::new(true)
    }

    fn new(variable: bool) -> Self {
        Self {
            variable,
            models: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            state: ListState::new(ROWS, ListAlignment::Top, px(OVERDRAW)),
        }
    }

    /// Scroll offset (px) the scrollbar-path frames set at frame `f`: 0, 40, ..., 6,000, ..., 0.
    fn sweep_offset(f: u64) -> f32 {
        let phase = ((f - UPDATES_END) % 300) as i64;
        (40 * (150 - (phase - 150).abs())) as f32
    }
}

fn edit_suffix(edits: u32) -> String {
    if edits > 0 {
        format!(" (edit {edits})")
    } else {
        String::new()
    }
}

/// A row of the uniform scene: 48 px, a label and a "Reply" button.
fn uniform_row(i: usize, edits: u32) -> AnyElement {
    div()
        .id(("row", i))
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .h(px(48.0))
        .px(px(16.0))
        .gap(px(12.0))
        .border_b(px(1.0))
        .border_color(hex(SEPARATOR))
        .bg(hex(WHITE))
        .hover(|style| style.bg(hex(SURFACE)))
        .child(div().flex_1().whitespace_nowrap().child(format!(
            "Message {i}: lorem ipsum dolor sit amet{}",
            edit_suffix(edits)
        )))
        .child(
            div()
                .flex()
                .w(px(72.0))
                .h(px(28.0))
                .flex_none()
                .justify_center()
                .items_center()
                .rounded(px(4.0))
                .border_1()
                .border_color(hex(BORDER))
                .bg(hex(SURFACE))
                .child("Reply"),
        )
        .into_any_element()
}

/// A row of the variable scene: paragraph, code block or table-like, by `i mod 3`.
fn variable_row(i: usize, edits: u32) -> AnyElement {
    let base = div()
        .id(("row", i))
        .w_full()
        .min_h(px(48.0))
        .px(px(16.0))
        .py(px(8.0))
        .border_b(px(1.0))
        .border_color(hex(SEPARATOR));
    match i % 3 {
        0 => {
            let sentence = "The quick brown fox jumps over the lazy dog. ";
            let text = format!("{}{}", sentence.repeat(2 + (5 * i) % 7), edit_suffix(edits));
            base.bg(hex(WHITE))
                .hover(|style| style.bg(hex(SURFACE)))
                .line_height(px(20.0))
                .child(text)
                .into_any_element()
        }
        1 => base
            .bg(hex(SURFACE))
            .hover(|style| style.bg(hex(CODE_HOVER)))
            .font_family(MONO)
            .text_size(px(13.0))
            .line_height(px(16.0))
            .child(div().child(format!("let value_{i} = compute({i}, {});", 3 * i)))
            .child(div().child(format!(
                "let value_{} = compute({}, {});",
                i + 1,
                i + 1,
                3 * i + 3
            )))
            .child(div().child(format!("let total_{i} = value_{i} + value_{};", i + 1)))
            .into_any_element(),
        _ => {
            let cell = |text: String| {
                div()
                    .flex_none()
                    .w(px(212.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .border_1()
                    .border_color(hex(BORDER))
                    .child(text)
            };
            base.bg(hex(WHITE))
                .hover(|style| style.bg(hex(SURFACE)))
                .flex()
                .flex_row()
                .child(cell(format!("Row {i}{}", edit_suffix(edits))))
                .child(cell("lorem ipsum dolor sit amet consectetur".to_string()))
                .child(cell(format!("{}", (37 * i) % 1000)))
                .child(cell("adipiscing elit sed do eiusmod tempor".to_string()))
                .into_any_element()
        }
    }
}

impl Scene for RowList {
    fn size(&self) -> (f32, f32) {
        (880.0, 480.0)
    }

    fn frame(&mut self, f: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        if self.models.is_empty() {
            self.models = (0..ROWS).map(|_| cx.new(|_| Edits(0))).collect();
        }
        let position = point(px(440.0), px(240.0));
        match f {
            0 => window.defer(cx, move |window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
            }),
            WARM..WHEEL_UP_END => {
                let delta_y = if f < WHEEL_DOWN_END { -40.0 } else { 40.0 };
                window.defer(cx, move |window, cx| {
                    window.dispatch_event(
                        ScrollWheelEvent {
                            position,
                            delta: ScrollDelta::Pixels(point(px(0.0), px(delta_y))),
                            modifiers: Modifiers::default(),
                            touch_phase: TouchPhase::Moved,
                        }
                        .to_platform_input(),
                        cx,
                    );
                });
            }
            WHEEL_UP_END..UPDATES_END => {
                let model = self.models[((f - WHEEL_UP_END) % ROWS as u64) as usize].clone();
                window.defer(cx, move |_, cx| {
                    model.update(cx, |edits, cx| {
                        edits.0 += 1;
                        cx.notify();
                    });
                });
            }
            UPDATES_END..SWEEP_END => {
                let offset = point(px(0.0), px(-Self::sweep_offset(f)));
                if self.variable {
                    self.state.set_offset_from_scrollbar(offset);
                } else {
                    self.scroll.set_offset_from_scrollbar(offset);
                }
            }
            _ => {}
        }

        let models = self.models.clone();
        let rows = if self.variable {
            let state = self.state.clone();
            list(state, move |ix, _window, cx| {
                variable_row(ix, models[ix].read(cx).0)
            })
            .id("rows")
            .size_full()
            .into_any_element()
        } else {
            uniform_list("rows", ROWS, move |range, _window, cx| {
                range
                    .map(|ix| uniform_row(ix, models[ix].read(cx).0))
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.scroll)
            .size_full()
            .into_any_element()
        };
        div()
            .size_full()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(rows)
            .into_any_element()
    }
}

register_scene!("PERF-PRF-005", RowList::uniform);
register_scene!("PERF-PRF-005-2", RowList::variable);
