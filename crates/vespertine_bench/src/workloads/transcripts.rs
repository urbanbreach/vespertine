//! 160- and 200-message rich transcripts (R4, R5): trackpad sweeps, dwells, jumps and a
//! highlight, over a list that follows its tail and a footer that reads the scroll state.

use super::{BORDER, INK, MONO, SANS, SURFACE, WHITE, hex};
use crate::harness::{Driver, Host, Stage};
use crate::workload::{Family, Workload};
use anyhow::Result;
use vespertine::{
    AnyView, AppContext as _, Context, Entity, FollowMode, FontWeight, IntoElement, ListAlignment,
    ListOffset, ListState, Modifiers, MouseMoveEvent, ParentElement as _, PlatformInput, Render,
    ScrollDelta, ScrollWheelEvent, SharedString, Styled as _, TouchPhase, Window, div, list, point,
    px,
};

/// Frames of one scroll cycle: a sweep, a dwell, the reverse sweep and a dwell.
pub const CYCLE: u64 = 300;
/// Frames of one sweep.
pub const SWEEP: u64 = 120;
/// Frames of one dwell at the end of a sweep.
pub const DWELL: u64 = 30;
/// Trackpad events per frame.
pub const EVENTS_PER_FRAME: usize = 4;
/// Logical pixels one trackpad event moves.
pub const EVENT_DELTA: f32 = 10.0;
/// Frames between jumps.
pub const JUMP_PERIOD: u64 = 240;
/// Frames a highlight lasts.
pub const HIGHLIGHT_FRAMES: u64 = 108;

/// What the transcript script does at one frame.
#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptPattern {
    /// Vertical deltas of the trackpad events of this frame (negative scrolls toward the end).
    pub events: Vec<f32>,
    /// The message to jump to, on the first frame of each jump period after the first.
    pub jump: Option<usize>,
    /// The highlighted message and its level in `0..=100` while a highlight runs.
    pub highlight: Option<(usize, u32)>,
}

impl TranscriptPattern {
    /// The pattern of `frame` for a transcript of `messages` messages.
    pub fn at(frame: u64, messages: usize) -> Self {
        let phase = frame % CYCLE;
        let direction = if phase < SWEEP {
            Some(-EVENT_DELTA)
        } else if (SWEEP + DWELL..SWEEP + DWELL + SWEEP).contains(&phase) {
            Some(EVENT_DELTA)
        } else {
            None
        };
        let epoch = frame / JUMP_PERIOD;
        let target = (epoch as usize * 37 + 11) % messages;
        let since_jump = frame % JUMP_PERIOD;
        let highlight = (since_jump < HIGHLIGHT_FRAMES).then(|| {
            // A triangle: up for half the highlight, down for the other half.
            let half = HIGHLIGHT_FRAMES / 2;
            let level = if since_jump < half {
                since_jump
            } else {
                HIGHLIGHT_FRAMES - since_jump
            };
            (target, (level * 100 / half) as u32)
        });
        Self {
            events: direction.map_or_else(Vec::new, |delta| vec![delta; EVENTS_PER_FRAME]),
            jump: (frame > 0 && since_jump == 0).then_some(target),
            highlight,
        }
    }
}

const WORDS: [&str; 24] = [
    "the", "render", "pass", "retained", "layout", "node", "scroll", "offset", "frame", "cache",
    "view", "paint", "shaped", "line", "list", "row", "measure", "replay", "update", "model",
    "style", "glyph", "atlas", "tile",
];

fn sentence(seed: usize, words: usize) -> String {
    let mut state = (seed as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut out = String::new();
    for ix in 0..words {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        if ix > 0 {
            out.push(' ');
        }
        out.push_str(WORDS[(state % WORDS.len() as u64) as usize]);
    }
    out.push('.');
    out
}

struct Message {
    index: usize,
    /// Highlight level in `0..=100`.
    highlight: u32,
}

impl Render for Message {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let m = self.index;
        let mut block = div()
            .w_full()
            .p(px(12.0))
            .border_color(hex(BORDER))
            .flex()
            .gap(px(6.0))
            .flex_col()
            .border_b_1()
            .child(
                div()
                    .gap(px(8.0))
                    .flex()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(if m.is_multiple_of(2) {
                                "Assistant"
                            } else {
                                "You"
                            }),
                    )
                    .child(
                        div()
                            .text_color(hex(0x57606a))
                            .child(format!("message {m}")),
                    ),
            )
            .child(
                div()
                    .line_height(px(20.0))
                    .child(SharedString::from(sentence(m, 18 + (m * 7) % 40))),
            );
        if m % 4 == 1 {
            block = block.child(div().flex().flex_col().gap(px(2.0)).children((0..3).map(
                |item| {
                    div()
                        .pl(px(12.0))
                        .child(format!("• {}", sentence(m * 10 + item, 6)))
                },
            )));
        }
        if m % 3 == 2 {
            block = block.child(
                div()
                    .flex()
                    .flex_col()
                    .bg(hex(SURFACE))
                    .rounded(px(6.0))
                    .p(px(8.0))
                    .font_family(MONO)
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .children((0..6).map(|line| {
                        div().whitespace_nowrap().child(format!(
                            "let value_{line} = frame.layout({m}, {line}); // {}",
                            sentence(m + line, 3)
                        ))
                    })),
            );
        }
        if m % 5 == 3 {
            block = block.child(
                div()
                    .line_height(px(20.0))
                    .child(SharedString::from(sentence(m + 9, 55))),
            );
        }
        if self.highlight > 0 {
            let alpha = self.highlight as f32 / 100.0 * 0.35;
            block = block.bg(vespertine::hsla(0.12, 0.9, 0.55, alpha));
        }
        block
    }
}

struct Footer {
    state: ListState,
}

impl Render for Footer {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let at_end = self.state.is_scrolled_to_end().unwrap_or(false);
        let top = self.state.logical_scroll_top();
        div()
            .h(px(28.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .bg(hex(SURFACE))
            .child(if at_end {
                "Following the latest message".to_string()
            } else {
                format!(
                    "Reading message {} +{:.0}px",
                    top.item_ix,
                    f32::from(top.offset_in_item)
                )
            })
    }
}

struct Transcript {
    messages: Vec<Entity<Message>>,
    state: ListState,
    footer: Entity<Footer>,
}

impl Render for Transcript {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let messages = self.messages.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hex(WHITE))
            .font_family(SANS)
            .text_size(px(14.0))
            .text_color(hex(INK))
            .child(
                list(self.state.clone(), move |ix, _, _| {
                    messages[ix].clone().into_any_element()
                })
                .flex_1(),
            )
            .child(self.footer.clone())
    }
}

struct TranscriptDriver {
    messages: Vec<Entity<Message>>,
    state: ListState,
    footer: Entity<Footer>,
    /// The highlighted message and level currently shown.
    shown: Option<(usize, u32)>,
}

impl Driver for TranscriptDriver {
    fn step(&mut self, frame: u64, stage: &mut Stage<'_>) {
        let pattern = TranscriptPattern::at(frame, self.messages.len());
        let position = point(px(440.0), px(240.0));
        for delta in &pattern.events {
            stage.dispatch(PlatformInput::ScrollWheel(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.0), px(*delta))),
                modifiers: Modifiers::default(),
                touch_phase: TouchPhase::Moved,
            }));
        }
        let (messages, state, footer, shown) =
            (&self.messages, &self.state, &self.footer, &mut self.shown);
        stage.mutate(|app| {
            if let Some(target) = pattern.jump {
                state.scroll_to(ListOffset {
                    item_ix: target,
                    offset_in_item: px(0.0),
                });
            }
            if *shown != pattern.highlight {
                if let Some((message, _)) = *shown {
                    messages[message].update(app, |message, cx| {
                        message.highlight = 0;
                        cx.notify();
                    });
                }
                if let Some((message, level)) = pattern.highlight {
                    messages[message].update(app, |message, cx| {
                        message.highlight = level;
                        cx.notify();
                    });
                }
                *shown = pattern.highlight;
            }
            footer.update(app, |_, cx| cx.notify());
        });
    }
}

fn setup(count: usize) -> impl Fn(&mut dyn Host) -> Result<Box<dyn Driver>> {
    move |harness| {
        let state = ListState::new(count, ListAlignment::Top, px(400.0));
        state.set_follow_mode(FollowMode::Tail);
        let (mut messages, mut footer) = (Vec::new(), None);
        harness.open_window(|_, cx| {
            messages = (0..count)
                .map(|index| {
                    cx.new(|_| Message {
                        index,
                        highlight: 0,
                    })
                })
                .collect();
            let footer_entity = cx.new(|_| Footer {
                state: state.clone(),
            });
            footer = Some(footer_entity.clone());
            AnyView::from(cx.new(|_| Transcript {
                messages: messages.clone(),
                state: state.clone(),
                footer: footer_entity,
            }))
        })?;
        harness.read_window(|window, cx| {
            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(440.0), px(240.0)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );
        })?;
        Ok(Box::new(TranscriptDriver {
            messages,
            state,
            footer: footer.expect("the window built the footer"),
            shown: None,
        }))
    }
}

/// The transcript workloads.
pub fn workloads() -> Vec<Workload> {
    vec![
        Workload {
            name: "transcript-160",
            family: Family::Transcripts,
            description: "160 rich messages; four trackpad events (40 px) per frame, 120-frame sweeps, 30-frame dwells, a jump every 240 frames, a 108-frame highlight",
            warmup: 30,
            frames: 200,
            setup: |harness| setup(160)(harness),
        },
        Workload {
            name: "transcript-200",
            family: Family::Transcripts,
            description: "200 rich messages with the same trackpad, dwell, jump and highlight pattern",
            warmup: 30,
            frames: 200,
            setup: |harness| setup(200)(harness),
        },
    ]
}
