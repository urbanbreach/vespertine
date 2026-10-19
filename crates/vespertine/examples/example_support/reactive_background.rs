//! Views shared by the `reactive_background` example and the APP-002 A1 test.
//!
//! [`BackgroundResult`] publishes a value computed after a background timer, while the
//! independent [`InputCounter`] keeps counting clicks and keystrokes during the wait.

use std::time::Duration;

use vespertine::{
    App, Context, Entity, FocusHandle, Focusable, KeyBinding, SharedString, Task, Window, actions,
    div, prelude::*, px, rgb,
};

actions!(reactive_background, [CountInput]);

pub const COUNTER_KEY_CONTEXT: &str = "InputCounter";

/// Binds the keystroke that increments the [`InputCounter`].
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new(
        "space",
        CountInput,
        Some(COUNTER_KEY_CONTEXT),
    )]);
}

/// Counts clicks and keystrokes; it never waits on background work.
pub struct InputCounter {
    pub count: usize,
    focus_handle: FocusHandle,
}

impl InputCounter {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            count: 0,
            focus_handle: cx.focus_handle(),
        }
    }

    fn increment(&mut self, cx: &mut Context<Self>) {
        self.count += 1;
        cx.notify();
    }
}

impl Focusable for InputCounter {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InputCounter {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("input-counter")
            .key_context(COUNTER_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &CountInput, _, cx| this.increment(cx)))
            .on_click(cx.listener(|this, _, _, cx| this.increment(cx)))
            .w(px(240.))
            .h(px(60.))
            .p_2()
            .bg(rgb(0x2d4a6b))
            .text_color(rgb(0xffffff))
            .child(SharedString::from(format!(
                "inputs counted: {} (click or press space)",
                self.count
            )))
    }
}

/// Shows a value published by a background task once its timer elapsed.
pub struct BackgroundResult {
    pub value: Option<u32>,
    // Owning the task means dropping this view cancels the pending result.
    work: Option<Task<()>>,
}

impl BackgroundResult {
    pub fn new() -> Self {
        Self {
            value: None,
            work: None,
        }
    }

    /// Sleeps `delay` on the background executor, then publishes `value` back to this view on
    /// the UI executor. Input handling and painting continue while the timer runs.
    pub fn start(&mut self, delay: Duration, value: u32, cx: &mut Context<Self>) {
        let executor = cx.background_executor().clone();
        let background = cx.background_spawn(async move {
            executor.timer(delay).await;
            value
        });
        self.work = Some(cx.spawn(async move |this, cx| {
            let value = background.await;
            this.update(cx, |this, cx| {
                this.value = Some(value);
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for BackgroundResult {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let text = match self.value {
            Some(value) => format!("background result: {value}"),
            None => "background result: waiting...".to_string(),
        };
        div()
            .w(px(240.))
            .h(px(60.))
            .p_2()
            .bg(rgb(0x4a2d6b))
            .text_color(rgb(0xffffff))
            .child(SharedString::from(text))
    }
}

/// Root view stacking the two independent views.
pub struct ReactiveDemo {
    pub counter: Entity<InputCounter>,
    pub result: Entity<BackgroundResult>,
}

impl ReactiveDemo {
    pub fn new(delay: Duration, value: u32, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let counter = cx.new(InputCounter::new);
        let result = cx.new(|_| BackgroundResult::new());
        result.update(cx, |result, cx| result.start(delay, value, cx));
        counter.focus_handle(cx).focus(window, cx);
        Self { counter, result }
    }
}

impl Render for ReactiveDemo {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.counter.clone())
            .child(self.result.clone())
    }
}
