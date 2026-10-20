//! PERF-STREAM: streaming chat transcript. Also supplies the PERF-IDLE content.

use std::cell::RefCell;
use std::rc::Rc;

use crate::common::{BORDER, INK, MONO, SANS, SURFACE, WHITE, hex};
use crate::corpus;
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, FollowMode, FontWeight, IntoElement, ListAlignment, ListState, ParentElement,
    SharedString, Styled, Window, div, list, px,
};

const PREFILLED: usize = 50;
const TOKENS_PER_PREFILLED: usize = 50;
const LIVE_TOKEN_BASE: usize = 5000;
const LIVE_PERIOD: u64 = 240;
const WIDTH: f32 = 800.0;
const HEIGHT: f32 = 500.0;
const OVERDRAW: f32 = 400.0;

struct Message {
    body: SharedString,
    /// Corpus lines `3m ..= 3m + 5` when `m % 3 == 2`.
    code: Vec<SharedString>,
}

impl Message {
    fn new(m: usize, body: String) -> Self {
        let code = if m % 3 == 2 {
            (3 * m..3 * m + 6).map(|k| corpus::line(k).into()).collect()
        } else {
            Vec::new()
        };
        Self {
            body: body.into(),
            code,
        }
    }
}

fn message_element(m: usize, message: &Message) -> AnyElement {
    let mut block = div()
        .w_full()
        .flex()
        .flex_col()
        .p(px(12.0))
        .gap(px(6.0))
        .border_b(px(1.0))
        .border_color(hex(BORDER))
        .text_color(hex(INK))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::BOLD)
                .child(format!("Message {m}")),
        )
        .child(
            div()
                .text_size(px(14.0))
                .line_height(px(20.0))
                .child(message.body.clone()),
        );
    if !message.code.is_empty() {
        let mut code = div()
            .flex()
            .flex_col()
            .bg(hex(SURFACE))
            .rounded(px(6.0))
            .p(px(8.0))
            .font_family(MONO)
            .text_size(px(12.0))
            .line_height(px(16.0));
        for line in &message.code {
            code = code.child(div().whitespace_nowrap().child(line.clone()));
        }
        block = block.child(code);
    }
    block.into_any_element()
}

/// Transcript state: 50 pre-filled messages followed by a live message.
pub struct StreamScene {
    messages: Rc<RefCell<Vec<Message>>>,
    list: ListState,
    /// Frame whose update has been applied, if any.
    applied: Option<u64>,
    /// Body text of the live message.
    live_body: String,
}

impl StreamScene {
    /// The state before any update: 50 pre-filled messages and an empty live message.
    pub fn new() -> Self {
        let messages = Rc::new(RefCell::new(Vec::new()));
        let list = ListState::new(0, ListAlignment::Bottom, px(OVERDRAW));
        let mut scene = Self {
            messages,
            list,
            applied: None,
            live_body: String::new(),
        };
        scene.reset();
        scene
    }

    fn reset(&mut self) {
        let mut messages = Vec::with_capacity(PREFILLED + 8);
        for m in 0..PREFILLED {
            let body: String = (TOKENS_PER_PREFILLED * m..TOKENS_PER_PREFILLED * (m + 1))
                .map(corpus::token)
                .collect();
            messages.push(Message::new(m, body));
        }
        messages.push(Message::new(PREFILLED, String::new()));
        let count = messages.len();
        *self.messages.borrow_mut() = messages;
        self.list.reset(count);
        self.list.set_follow_mode(FollowMode::Tail);
        self.live_body.clear();
        self.applied = None;
    }

    fn apply(&mut self, f: u64) {
        let live = self.messages.borrow().len() - 1;
        self.live_body
            .push_str(&corpus::token(LIVE_TOKEN_BASE + f as usize));
        self.messages.borrow_mut()[live].body = SharedString::from(self.live_body.clone());
        self.list.remeasure_items(live..live + 1);
        if f > 0 && f.is_multiple_of(LIVE_PERIOD) {
            self.messages
                .borrow_mut()
                .push(Message::new(live + 1, String::new()));
            self.list.splice(live + 1..live + 1, 1);
            self.live_body.clear();
        }
        self.applied = Some(f);
    }

    /// Brings the transcript to the state after the update of frame `f`.
    fn advance_to(&mut self, f: u64) {
        match self.applied {
            Some(done) if done == f => {}
            Some(done) if done < f => {
                for next in done + 1..=f {
                    self.apply(next);
                }
            }
            _ => {
                self.reset();
                for next in 0..=f {
                    self.apply(next);
                }
            }
        }
    }

    /// Builds the window content for the current transcript state.
    pub fn element(&self) -> AnyElement {
        let messages = self.messages.clone();
        div()
            .size_full()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .child(
                list(self.list.clone(), move |m, _window, _cx| {
                    message_element(m, &messages.borrow()[m])
                })
                .size_full(),
            )
            .into_any_element()
    }

    pub fn size() -> (f32, f32) {
        (WIDTH, HEIGHT)
    }
}

impl Scene for StreamScene {
    fn size(&self) -> (f32, f32) {
        Self::size()
    }

    fn frame(&mut self, f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        self.advance_to(f);
        self.element()
    }
}

register_scene!("PERF-STREAM", StreamScene::new);
