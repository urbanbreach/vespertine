//! ANM-002 A5: an input that docks and returns between two slots and is reversed mid-flight
//! while text is typed into it.
//!
//! - The input rides a route glide (`Travel`): changing its slot jumps its layout position, but
//!   the displayed surface glides from where it visibly is, and reversing mid-flight keeps its
//!   position and velocity. Its caret, text and hitbox all live inside the displaced surface.
//! - The sibling slots never move while it travels.
//! - Resizing the unrelated left pane (a declarative motion track) reflows the layout; the input
//!   absorbs that in a separate reflow glide that never restarts the route glide.
//!
//! The demo drives itself for a few seconds (reversing the input every 300 ms while typing and
//! resizing the left pane), then hands over: Space reverses the input, Tab resizes the pane,
//! typing edits the text, Escape quits.

use std::time::Duration;

use vespertine::{
    App, Bounds, Context, FocusHandle, Focusable, KeyDownEvent, Window, WindowBounds,
    WindowOptions, div,
    motion::{MotionDescription, MotionExt as _, MotionInitial, MotionProps, presets},
    prelude::*,
    px, rgb, size,
};
use vespertine_platform::application;

const SLOT_SIZE: (f32, f32) = (300.0, 56.0);
const SLOT_TOP: [f32; 2] = [80.0, 260.0];

struct MotionRetarget {
    focus_handle: FocusHandle,
    text: String,
    /// Which slot the input lives in (0 = top, 1 = bottom); a change is a route change.
    slot: usize,
    /// Whether the unrelated left pane is wide.
    wide_pane: bool,
}

impl Focusable for MotionRetarget {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MotionRetarget {
    fn reverse(&mut self, cx: &mut Context<Self>) {
        self.slot = 1 - self.slot;
        cx.notify();
    }

    fn resize_pane(&mut self, cx: &mut Context<Self>) {
        self.wide_pane = !self.wide_pane;
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "space" => self.reverse(cx),
            "tab" => self.resize_pane(cx),
            "escape" => cx.quit(),
            "backspace" => {
                self.text.pop();
                cx.notify();
            }
            _ => {
                if let Some(text) = event.keystroke.key_char.as_ref() {
                    self.text.push_str(text);
                    cx.notify();
                }
            }
        }
    }
}

impl Render for MotionRetarget {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The left pane's width is a native declarative track: no re-render per frame.
        let pane_width = if self.wide_pane { 220.0 } else { 120.0 };
        let pane = div()
            .h_full()
            .bg(rgb(0x30343d))
            .text_color(rgb(0xf0f1f3))
            .p_3()
            .child("pane (Tab resizes)")
            .with_motion(
                "pane",
                MotionDescription::new(MotionProps::new().width(pane_width))
                    .initial(MotionInitial::Style(MotionProps::new().width(120.0)))
                    .transition(presets::SIZE_TRANSITION.transition()),
            );

        let slot = |index: usize| {
            div()
                .absolute()
                .top(px(SLOT_TOP[index]))
                .left_0()
                .w(px(SLOT_SIZE.0))
                .h(px(SLOT_SIZE.1))
                .border_1()
                .border_dashed()
                .border_color(rgb(0x606674))
        };

        // The input's layout position jumps with its slot; the route glide hides the jump.
        let route_glide = if self.slot == 0 {
            presets::RETURN_GLIDE.duration()
        } else {
            presets::DOCKING_GLIDE.duration()
        };
        let input = div()
            .absolute()
            .top(px(SLOT_TOP[self.slot]))
            .left_0()
            .child(
                div()
                    .id("input")
                    .w(px(SLOT_SIZE.0))
                    .h(px(SLOT_SIZE.1))
                    .flex()
                    .items_center()
                    .px_3()
                    .gap_px()
                    .bg(rgb(0x405470))
                    .rounded_lg()
                    .text_color(rgb(0xf0f1f3))
                    .child(self.text.clone())
                    // The caret is part of the displaced surface.
                    .child(div().w(px(2.0)).h(px(24.0)).bg(rgb(0xffffff)))
                    .with_travel("input-travel", self.slot)
                    .route_duration(route_glide),
            );

        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .size_full()
            .bg(rgb(0x181a1f))
            .child(pane)
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .relative()
                    .child(slot(0))
                    .child(slot(1))
                    .child(input),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.0), px(420.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|cx| MotionRetarget {
                        focus_handle: cx.focus_handle(),
                        text: String::new(),
                        slot: 0,
                        wide_pane: false,
                    });
                    window.focus(&view.focus_handle(cx), cx);
                    view
                },
            )
            .unwrap();

        // Self-driving phase: reverse the input every 300 ms (mid-flight, since the glide lasts
        // 420-470 ms) while typing, and resize the unrelated pane every fourth step.
        let view = window.entity(cx).unwrap();
        cx.spawn(async move |cx| {
            for step in 0..12u8 {
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                view.update(cx, |this, cx| {
                    this.text.push((b'a' + step) as char);
                    this.reverse(cx);
                    if step % 4 == 3 {
                        this.resize_pane(cx);
                    }
                });
            }
        })
        .detach();
        cx.activate(true);
    });
}
