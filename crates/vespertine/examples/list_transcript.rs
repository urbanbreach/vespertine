//! A 160-message transcript with a "Back to bottom" control (PRF-005 A2).
//!
//! The control is a view of its own that reads only the list's at-end predicate and is animated
//! every frame. The example scrolls away from the bottom and back and checks, frame by frame,
//! that the list rows replay (no row is built) while the answer of the predicate is stable, that
//! a change of the answer rebuilds the control and not the list content, and that scrolling to
//! the end when the list already is at the end invalidates nothing.
//!
//! Run with: `cargo run -p vespertine --example list_transcript --features test-support`

use vespertine::{
    App, AppContext as _, Context, Entity, FrameStats, IntoElement, ListAlignment, ListState,
    Modifiers, ParentElement, Render, ScrollDelta, ScrollWheelEvent, Styled, TestAppContext,
    TouchPhase, VisualTestContext, Window, div, list, point, px, rgb, size,
};

const MESSAGES: usize = 160;

struct Message {
    text: String,
}

/// The control: reads the at-end predicate and nothing else of the list.
struct BackToBottom {
    state: ListState,
    frame: u32,
}

impl Render for BackToBottom {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let at_end = self.state.is_scrolled_to_end().unwrap_or(true);
        div()
            .px_3()
            .py_1()
            .rounded_md()
            .bg(if at_end { rgb(0xeeeeee) } else { rgb(0x2f6fed) })
            .opacity(0.6 + 0.4 * (self.frame % 20) as f32 / 20.0)
            .child(if at_end {
                "At the bottom"
            } else {
                "Back to bottom"
            })
    }
}

struct Transcript {
    state: ListState,
    messages: Vec<Entity<Message>>,
    button: Entity<BackToBottom>,
}

impl Render for Transcript {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let messages = self.messages.clone();
        div()
            .size_full()
            .relative()
            .child(
                list(self.state.clone(), move |ix, _, cx| {
                    div()
                        .px_3()
                        .py_1()
                        .child(messages[ix].read(cx).text.clone())
                        .into_any_element()
                })
                .id("transcript")
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(8.))
                    .right(px(8.))
                    .child(self.button.clone()),
            )
    }
}

fn frame(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn stats(cx: &mut VisualTestContext) -> FrameStats {
    cx.update(|window, _| window.frame_stats())
}

fn reset(cx: &mut VisualTestContext) {
    cx.update(|window, _| window.reset_frame_stats());
}

fn wheel(cx: &mut VisualTestContext, dy: f32) {
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(20.), px(20.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
        modifiers: Modifiers::default(),
        touch_phase: TouchPhase::Moved,
    });
    frame(cx);
}

/// One animation frame of the control: it notifies itself and nothing else.
fn animate(cx: &mut VisualTestContext, button: &Entity<BackToBottom>) {
    cx.update(|_, cx: &mut App| {
        button.update(cx, |button, cx| {
            button.frame += 1;
            cx.notify();
        })
    });
    frame(cx);
}

fn main() {
    let mut app = TestAppContext::single();
    let (transcript, cx) = app.add_window_view(|_, cx| {
        let messages: Vec<Entity<Message>> = (0..MESSAGES)
            .map(|ix| {
                cx.new(|_| Message {
                    text: format!(
                        "Message {ix}: {}",
                        "the quick brown fox ".repeat(1 + ix % 4)
                    ),
                })
            })
            .collect();
        let state = ListState::new(MESSAGES, ListAlignment::Bottom, px(0.)).measure_all();
        let button = cx.new(|_| BackToBottom {
            state: state.clone(),
            frame: 0,
        });
        Transcript {
            state,
            messages,
            button,
        }
    });
    cx.simulate_resize(size(px(360.), px(240.)));
    let (state, button) = transcript.read_with(cx, |t, _| (t.state.clone(), t.button.clone()));
    for _ in 0..4 {
        frame(cx);
    }
    assert_eq!(
        state.is_scrolled_to_end(),
        Some(true),
        "the transcript starts at the bottom"
    );

    // At the bottom, the control animates: only it is built, the list rows replay.
    reset(cx);
    for _ in 0..30 {
        animate(cx, &button);
    }
    let animating = stats(cx);
    println!("30 animation frames at the bottom: {animating:?}");
    assert_eq!(
        animating.rows_built, 0,
        "no row is built while the control animates"
    );
    assert_eq!(
        animating.views_built, 30,
        "only the control is built, once per frame"
    );

    // Scroll away from the bottom: the answer flips once, the control rebuilds for it.
    let mut flips = 0;
    let mut last = state.is_scrolled_to_end();
    let mut worst_rows = 0;
    for _ in 0..40 {
        reset(cx);
        wheel(cx, 15.);
        let now = state.is_scrolled_to_end();
        flips += usize::from(now != last);
        last = now;
        let frame_stats = stats(cx);
        worst_rows = worst_rows.max(frame_stats.rows_built);
        if now == Some(false) && frame_stats.rows_built == 0 {
            animate(cx, &button);
        }
    }
    println!(
        "scrolled away: the answer flipped {flips} time(s), at most {worst_rows} rows per frame"
    );
    assert_eq!(state.is_scrolled_to_end(), Some(false));
    assert_eq!(flips, 1);
    assert!(worst_rows <= 4, "scrolling builds only the rows that enter");

    // Away from the bottom with a stable answer: animation frames build only the control.
    reset(cx);
    for _ in 0..30 {
        animate(cx, &button);
    }
    let away = stats(cx);
    assert_eq!(away.rows_built, 0, "{away:?}");
    assert_eq!(away.views_built, 30, "{away:?}");

    // Back to the end: the answer flips back and the list builds only the rows that enter.
    let mut flips_back = 0;
    for _ in 0..80 {
        reset(cx);
        let before = state.is_scrolled_to_end();
        wheel(cx, -15.);
        flips_back += usize::from(before != state.is_scrolled_to_end());
        assert!(stats(cx).rows_built <= 4);
    }
    println!("scrolled back: the answer flipped {flips_back} time(s)");
    assert_eq!(state.is_scrolled_to_end(), Some(true));
    assert_eq!(flips_back, 1);

    // Scrolling to the end when already there invalidates nothing.
    for _ in 0..3 {
        frame(cx);
    }
    reset(cx);
    cx.update(|_, _| state.scroll_to_end());
    frame(cx);
    frame(cx);
    let settled = stats(cx);
    println!("scroll_to_end at the end: {settled:?}");
    assert_eq!(settled.rows_built, 0, "{settled:?}");
    assert_eq!(settled.views_built, 0, "{settled:?}");
    println!("the transcript replays whenever the at-end answer is stable");
}
