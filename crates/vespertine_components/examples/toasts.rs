//! CMP-018 A10: the toast stack.
//!
//! The window starts with one notice of each kind (information, success, error). Use the buttons
//! to publish more: hover the stack to pause every countdown and fan the cards out, drag a card to
//! the right past 45 px to dismiss it, deactivate the window to pause the countdowns. "Pending
//! error" fills a view's pending-error slot, which `take_error` publishes on the next turn.
//!
//! With `TOASTS_SCRIPT=1` the example runs the acceptance script with synthesized input and exits
//! 0 when every check passes (1 otherwise): the three start-up notices are live in order and the
//! window keeps receiving frames, hovering the stack pauses the countdown of the front notice and
//! leaving resumes it, a rightward swipe past 45 px dismisses the front card, the buttons
//! publish (a repeated error stays one card, a burst of five adds five notices, the pending
//! error appears) and every notice leaves by itself once its countdown ran out, which needs the
//! window to be active and its display to deliver frames: a blanked display or an inactive
//! window fails the first checks by name instead of stalling.

use std::{cell::Cell, rc::Rc, time::Duration};

use vespertine::{
    App, AppContext, AsyncApp, Bounds, Context, InteractiveElement, IntoElement, ParentElement,
    PlatformInput, Render, SharedString, StatefulInteractiveElement, Styled, Window, WindowBounds,
    WindowHandle, WindowOptions, div, point, px, size,
};
use vespertine::{
    Modifiers, MotionClock, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
};
use vespertine_components::{
    nav_theme::NavTheme,
    toast::{self, ToastKind, render_toaster, take_error},
};
use vespertine_platform::application;

const BUTTON: (f32, f32) = (112.0, 30.0);
const BUTTON_GAP: f32 = 6.0;
const PADDING: f32 = 16.0;
const TEXT_ROW: f32 = 20.0;
const ROW_GAP: f32 = 12.0;
const LABELS: [&str; 6] = [
    "Error",
    "Success",
    "Info",
    "Burst of five",
    "Same error twice",
    "Pending error",
];

struct Example {
    pending_error: Option<SharedString>,
    published: usize,
    /// Counts the renders of the window: every frame the platform delivered that redrew it.
    renders: Rc<Cell<usize>>,
}

fn button(
    label: &'static str,
    cx: &mut Context<Example>,
    on_click: impl Fn(&mut Example, &mut Window, &mut Context<Example>) + 'static,
) -> impl IntoElement {
    let theme = NavTheme::neutral();
    div()
        .id(label)
        .w(px(BUTTON.0))
        .h(px(BUTTON.1))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(theme.popup_surface)
        .border_1()
        .border_color(theme.popup_border)
        .cursor_pointer()
        .child(label)
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        // Publishing mutates global state, so a pending error is handed to the next turn.
        take_error(&mut self.pending_error, cx);
        let theme = NavTheme::neutral();
        let buttons = div()
            .flex()
            .flex_row()
            .gap(px(BUTTON_GAP))
            .child(button("Error", cx, |this, _, cx| {
                this.published += 1;
                toast::error(cx, format!("Something failed ({})", this.published));
            }))
            .child(button("Success", cx, |this, _, cx| {
                this.published += 1;
                toast::success(cx, format!("Saved ({})", this.published));
            }))
            .child(button("Info", cx, |this, _, cx| {
                this.published += 1;
                toast::info(cx, format!("Heads up ({})", this.published));
            }))
            .child(button("Burst of five", cx, |this, _, cx| {
                for _ in 0..5 {
                    this.published += 1;
                    toast::info(cx, format!("Notice number {}", this.published));
                }
            }))
            .child(button("Same error twice", cx, |_, _, cx| {
                toast::error(cx, "The same error");
                toast::error(cx, "The same error");
            }))
            .child(button("Pending error", cx, |this, _, cx| {
                this.pending_error = Some("Raised from a pending-error slot".into());
                cx.notify();
            }));
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(ROW_GAP))
            .p(px(PADDING))
            .bg(theme.background)
            .text_color(theme.text)
            .text_size(px(13.))
            .child(
                div()
                    .h(px(TEXT_ROW))
                    .child("Toasts stack in the bottom-right corner. Hover to pause and fan out; swipe right to dismiss."),
            )
            .child(buttons)
            .children(render_toaster(window, cx))
    }
}

/// The acceptance script: synthesized pointer input and checks of the queue and of the frames the
/// window received.
struct Script {
    cx: AsyncApp,
    window: WindowHandle<Example>,
    renders: Rc<Cell<usize>>,
    failures: Vec<String>,
}

impl Script {
    async fn sleep(&self, duration: Duration) {
        self.cx.background_executor().timer(duration).await;
    }

    fn check(&mut self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures.push(what.to_owned());
        }
    }

    fn send(&mut self, event: PlatformInput) {
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("the window is open");
    }

    fn mouse_move(&mut self, x: f32, y: f32, pressed: Option<MouseButton>) {
        self.send(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x), px(y)),
            pressed_button: pressed,
            modifiers: Modifiers::none(),
        }));
    }

    fn button_event(&mut self, x: f32, y: f32, down: bool) {
        let position = point(px(x), px(y));
        self.send(if down {
            PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            })
        } else {
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::none(),
                click_count: 1,
            })
        });
    }

    async fn click(&mut self, x: f32, y: f32) {
        self.mouse_move(x, y, None);
        self.sleep(Duration::from_millis(80)).await;
        self.button_event(x, y, true);
        self.button_event(x, y, false);
        self.sleep(Duration::from_millis(200)).await;
    }

    /// The center of the button called `label`, in window coordinates.
    fn button_center(label: &str) -> (f32, f32) {
        let index = LABELS
            .iter()
            .position(|candidate| *candidate == label)
            .expect("a known button");
        (
            PADDING + BUTTON.0 / 2.0 + index as f32 * (BUTTON.0 + BUTTON_GAP),
            PADDING + TEXT_ROW + ROW_GAP + BUTTON.1 / 2.0,
        )
    }

    fn viewport(&self) -> (f32, f32) {
        self.cx
            .clone()
            .update_window(self.window.into(), |_, window, _| {
                let viewport = window.viewport_size();
                (viewport.width.as_f32(), viewport.height.as_f32())
            })
            .expect("the window is open")
    }

    /// A point on the front card of the stack: its bottom-right corner is 24 px from the corner
    /// of the window and the front card is the lowest one.
    fn front_card(&self) -> (f32, f32) {
        let (width, height) = self.viewport();
        let stack_width = toast::model::stack_width(width);
        (width - 24.0 - stack_width / 2.0, height - 24.0 - 14.0)
    }

    fn messages(&self) -> Vec<String> {
        self.cx.update(|cx| {
            toast::live_messages(cx)
                .iter()
                .map(ToString::to_string)
                .collect()
        })
    }

    fn hovered(&self) -> bool {
        self.cx
            .update(|cx| toast::with_queue(cx, |queue| queue.is_hovered()))
            .unwrap_or(false)
    }

    /// The lifetime the oldest running notice has left, in milliseconds.
    fn oldest_remaining(&self) -> f64 {
        self.cx.update(|cx| {
            let now = MotionClock::now_ms(cx);
            toast::with_queue(cx, |queue| {
                queue
                    .toasts()
                    .first()
                    .map_or(f64::NAN, |toast| toast.remaining_at(now))
            })
            .unwrap_or(f64::NAN)
        })
    }

    fn active(&self) -> bool {
        self.cx
            .clone()
            .update_window(self.window.into(), |_, window, _| window.is_window_active())
            .unwrap_or(false)
    }

    async fn run(&mut self) {
        self.sleep(Duration::from_millis(600)).await;
        self.check(
            self.active(),
            "the window is active (an inactive window pauses every countdown)",
        );
        self.check(
            self.messages()
                == [
                    "Information: this notice lives four seconds",
                    "Success: everything was saved",
                    "Error: something went wrong",
                ],
            "the three start-up notices are live in publishing order",
        );
        let drawn = self.renders.get();
        println!("info {drawn} renders during the first 600 ms");
        self.check(
            drawn >= 5,
            "the entry glide of the start-up notices drew at least five frames (a blanked display delivers none)",
        );

        // Hovering the stack pauses the countdown and leaving resumes it.
        let (x, y) = self.front_card();
        self.mouse_move(x, y, None);
        self.sleep(Duration::from_millis(250)).await;
        self.check(self.hovered(), "the pointer over the stack hovers it");
        let paused = self.oldest_remaining();
        self.sleep(Duration::from_millis(500)).await;
        let still = self.oldest_remaining();
        self.check(
            (paused - still).abs() < 1.0,
            "hovering pauses the countdown of the oldest notice",
        );
        self.mouse_move(4.0, 4.0, None);
        self.sleep(Duration::from_millis(250)).await;
        self.check(!self.hovered(), "leaving the stack ends the hover");
        let resumed = self.oldest_remaining();
        self.sleep(Duration::from_millis(400)).await;
        self.check(
            self.oldest_remaining() < resumed - 200.0,
            "leaving resumes the countdown",
        );

        // A rightward swipe past 45 px dismisses the front card.
        let (x, y) = self.front_card();
        self.mouse_move(x, y, None);
        self.sleep(Duration::from_millis(300)).await;
        let before = self.messages();
        self.button_event(x, y, true);
        self.mouse_move(x + 20.0, y, Some(MouseButton::Left));
        self.sleep(Duration::from_millis(100)).await;
        self.mouse_move(x + 90.0, y, Some(MouseButton::Left));
        self.sleep(Duration::from_millis(150)).await;
        self.button_event(x + 90.0, y, false);
        self.mouse_move(4.0, 4.0, None);
        self.sleep(Duration::from_millis(500)).await;
        let after = self.messages();
        self.check(
            after.len() + 1 == before.len() && after == before[..before.len() - 1],
            "a swipe past 45 px dismisses exactly the front (newest) card",
        );

        // The buttons publish.
        let (x, y) = Self::button_center("Info");
        self.click(x, y).await;
        self.check(
            self.messages().last().map(String::as_str) == Some("Heads up (1)"),
            "the Info button publishes an information notice",
        );
        let count = self.messages().len();
        let (x, y) = Self::button_center("Same error twice");
        self.click(x, y).await;
        self.check(
            self.messages().len() == count + 1
                && self.messages().last().map(String::as_str) == Some("The same error"),
            "an error published twice stays one card",
        );
        let count = self.messages().len();
        let (x, y) = Self::button_center("Burst of five");
        self.click(x, y).await;
        self.check(
            self.messages().len() == count + 5,
            "the burst button adds five notices",
        );
        let (x, y) = Self::button_center("Pending error");
        self.click(x, y).await;
        self.sleep(Duration::from_millis(200)).await;
        self.check(
            self.messages()
                .iter()
                .any(|message| message == "Raised from a pending-error slot"),
            "the pending error is published on the next turn",
        );

        // Every notice leaves by itself once its countdown ran out: the timer wakes the window
        // and the queue is ticked by the redraw it asks for.
        let mut waited = 0;
        while waited < 6000 && !self.messages().is_empty() {
            self.sleep(Duration::from_millis(100)).await;
            waited += 100;
        }
        self.check(
            self.messages().is_empty(),
            "every notice expired by itself within its four-second lifetime",
        );
    }
}

fn main() {
    let script = std::env::var_os("TOASTS_SCRIPT").is_some_and(|value| value == "1");
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(760.), px(420.)), cx);
        let renders = Rc::new(Cell::new(0));
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                {
                    let renders = renders.clone();
                    move |_, cx| {
                        toast::info(cx, "Information: this notice lives four seconds");
                        toast::success(cx, "Success: everything was saved");
                        toast::show(cx, ToastKind::Error, "Error: something went wrong");
                        cx.new(|_| Example {
                            pending_error: None,
                            published: 0,
                            renders,
                        })
                    }
                },
            )
            .expect("open the window");
        cx.activate(true);
        if script {
            cx.spawn(async move |cx| {
                let mut script = Script {
                    cx: cx.clone(),
                    window,
                    renders,
                    failures: Vec::new(),
                };
                script.run().await;
                let code = if script.failures.is_empty() {
                    println!("toasts: all checks passed");
                    0
                } else {
                    println!("toasts: {} check(s) failed", script.failures.len());
                    1
                };
                std::process::exit(code);
            })
            .detach();
        }
    });
}
