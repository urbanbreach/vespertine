//! Focus-scoped shortcuts (INP-007 A1).
//!
//! One shortcut (Ctrl+K) is bound to a different action in each of two panes,
//! selected by which pane holds focus. Opening the modal (press M, or click
//! "Open modal") installs a context that binds Ctrl+K to the no-action marker,
//! which blocks the shortcut for both panes; closing the modal (Escape)
//! restores both.
//!
//! The example drives itself through a scripted sequence of focus changes and
//! keystrokes and prints what ran, then exits. Pass `--interactive` to keep
//! the window open and try it by hand: click a pane to focus it, then press
//! Ctrl+K, M and Escape.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::time::Duration;

use vespertine::{
    App, AsyncApp, Bounds, Context, FocusHandle, KeyBinding, Keystroke, NoAction, SharedString,
    Window, WindowBounds, WindowHandle, WindowOptions, actions, div, prelude::*, px, rgb, size,
};
use vespertine_platform::application;

actions!(
    focus_scoped_shortcuts,
    [PaneAAction, PaneBAction, OpenModal, CloseModal]
);

struct Example {
    pane_a: FocusHandle,
    pane_b: FocusHandle,
    modal: FocusHandle,
    modal_open: bool,
    /// The key context of the pane that last held focus; it stays in effect
    /// while the modal (which is not inside either pane) has focus.
    active: &'static str,
    log: Vec<SharedString>,
}

impl Example {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            pane_a: cx.focus_handle(),
            pane_b: cx.focus_handle(),
            modal: cx.focus_handle(),
            modal_open: false,
            active: "PaneA",
            log: Vec::new(),
        }
    }

    fn record(&mut self, line: &str, cx: &mut Context<Self>) {
        self.log.push(line.to_string().into());
        cx.notify();
    }
}

fn pane(name: &'static str, focus: &FocusHandle, window: &Window) -> impl IntoElement {
    div()
        .id(name)
        .track_focus(focus)
        .flex_1()
        .p_4()
        .border_2()
        .border_color(if focus.is_focused(window) {
            rgb(0x2563eb)
        } else {
            rgb(0xd1d5db)
        })
        .child(format!("{name}: press Ctrl+K"))
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pane_a.contains_focused(window, cx) {
            self.active = "PaneA";
        } else if self.pane_b.contains_focused(window, cx) {
            self.active = "PaneB";
        }
        div()
            .size_full()
            .key_context(self.active)
            .bg(rgb(0xffffff))
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .on_action(
                cx.listener(|this, _: &PaneAAction, _, cx| this.record("pane A: Ctrl+K", cx)),
            )
            .on_action(
                cx.listener(|this, _: &PaneBAction, _, cx| this.record("pane B: Ctrl+K", cx)),
            )
            .on_action(cx.listener(|this, _: &OpenModal, window, cx| {
                this.modal_open = true;
                window.focus(&this.modal, cx);
                this.record("modal opened", cx);
            }))
            .on_action(cx.listener(|this, _: &CloseModal, window, cx| {
                this.modal_open = false;
                window.focus(&this.pane_a, cx);
                this.record("modal closed", cx);
            }))
            .child(
                div()
                    .h(px(120.))
                    .gap_2()
                    .flex_row()
                    .flex()
                    .child(pane("Pane A", &self.pane_a, window))
                    .child(pane("Pane B", &self.pane_b, window)),
            )
            .child(
                div()
                    .id("open-modal")
                    .px_2()
                    .border_1()
                    .child("Open modal (M)")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenModal), cx)),
            )
            .when(self.modal_open, |this| {
                // The modal's context binds Ctrl+K to the no-action marker, so
                // neither pane's binding runs while it has focus.
                this.child(
                    div()
                        .id("modal")
                        .key_context("Modal")
                        .track_focus(&self.modal)
                        .p_4()
                        .border_2()
                        .border_color(rgb(0xdc2626))
                        .child("Modal: Ctrl+K is blocked. Escape closes."),
                )
            })
            .children(self.log.iter().map(|line| div().child(line.clone())))
    }
}

fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("ctrl-k", PaneAAction, Some("PaneA")),
        KeyBinding::new("ctrl-k", PaneBAction, Some("PaneB")),
        KeyBinding::new("m", OpenModal, Some("PaneA || PaneB")),
        // Registered later and bound in the more specific context: the
        // no-action marker suppresses the outranked pane bindings while the
        // modal context is active.
        KeyBinding::new("ctrl-k", NoAction, Some("Modal")),
        KeyBinding::new("escape", CloseModal, Some("Modal")),
    ]
}

async fn press(window: WindowHandle<Example>, cx: &mut AsyncApp, keys: &str) {
    for source in keys.split(' ') {
        let keystroke = Keystroke::parse(source).expect("valid keystroke");
        // Not `WindowHandle::update`: it leases the root view, and a keystroke dispatched into a
        // dirty window draws it first, which needs that lease.
        cx.update_window(window.into(), |_, window, cx| {
            window.dispatch_keystroke(keystroke, cx)
        })
        .expect("window is open");
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
}

async fn focus(
    window: WindowHandle<Example>,
    cx: &mut AsyncApp,
    pane: fn(&Example) -> &FocusHandle,
) {
    window
        .update(cx, |view, window, cx| window.focus(pane(view), cx))
        .expect("window is open");
    cx.background_executor()
        .timer(Duration::from_millis(100))
        .await;
}

fn run_example() {
    let interactive = std::env::args().any(|argument| argument == "--interactive");
    application().run(move |cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        cx.bind_keys(bindings());
        let bounds = Bounds::centered(None, size(px(560.), px(420.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(Example::new),
            )
            .unwrap();
        cx.activate(true);

        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            // The same shortcut runs a different action per focused pane.
            focus(window, cx, |view| &view.pane_a).await;
            press(window, cx, "ctrl-k").await;
            focus(window, cx, |view| &view.pane_b).await;
            press(window, cx, "ctrl-k").await;
            // Opening the modal blocks it (here over pane B)...
            press(window, cx, "m ctrl-k").await;
            // ...and closing it restores it (focus returns to pane A)...
            press(window, cx, "escape ctrl-k").await;
            // ...also blocking it over pane A, and restoring it for pane B.
            press(window, cx, "m ctrl-k escape").await;
            focus(window, cx, |view| &view.pane_b).await;
            press(window, cx, "ctrl-k").await;

            let log = window
                .read_with(cx, |view, _| view.log.clone())
                .expect("window is open");
            for line in &log {
                println!("{line}");
            }
            let expected = [
                "pane A: Ctrl+K",
                "pane B: Ctrl+K",
                "modal opened",
                "modal closed",
                "pane A: Ctrl+K",
                "modal opened",
                "modal closed",
                "pane B: Ctrl+K",
            ];
            let as_expected = log.iter().map(|line| line.as_ref()).eq(expected);
            println!(
                "{}",
                if as_expected {
                    "OK: the shortcut ran per focused pane, was blocked by the modal and restored after it"
                } else {
                    "UNEXPECTED sequence"
                }
            );
            if !interactive {
                cx.update(|cx| cx.quit());
            }
        })
        .detach();
    });
}

fn main() {
    run_example();
}
