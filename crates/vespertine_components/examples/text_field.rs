//! CMP-007 A1: the single-line text field.
//!
//! Interactively the window shows a field, an event log and a button that opens an anchored
//! popup (Escape or the Close button closes it). With `TEXT_FIELD_SCRIPT=1` the example runs the
//! acceptance script and exits 0 when every check passes (1 otherwise): Enter submits the exact
//! value, Escape cancels, Ctrl+A selects all, "café ☕ 日本語" is pasted, and an anchored popup
//! is reopened several times with the field's text, selection and focus unchanged afterwards.

use std::time::Duration;
use vespertine::{
    App, AppContext, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, Keystroke, MouseButton, ParentElement, Render, Styled,
    Subscription, Window, WindowBounds, WindowHandle, WindowOptions, div, px, rgb, size,
};
use vespertine_components::text::{TextField, TextFieldEvent, actions::bind_default_keys};
use vespertine_platform::application;

struct Example {
    field: Entity<TextField>,
    popup: bool,
    popup_focus: FocusHandle,
    log: Vec<String>,
    _subscription: Subscription,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            TextField::new(window, cx)
                .placeholder("Type here")
                .value("hello")
        });
        let subscription = cx.subscribe(&field, |this, _, event: &TextFieldEvent, cx| {
            let line = match event {
                TextFieldEvent::Changed(value) => format!("changed: {value}"),
                TextFieldEvent::Submitted(value) => format!("submitted: {value}"),
                TextFieldEvent::Cancelled => "cancelled".to_owned(),
                TextFieldEvent::PastedPaths(paths) => format!("pasted {} path(s)", paths.len()),
                TextFieldEvent::PastedImages(images) => format!("pasted {} image(s)", images.len()),
                TextFieldEvent::Focused => "focused".to_owned(),
                TextFieldEvent::Blurred => "blurred".to_owned(),
                TextFieldEvent::KeyDown(_) | TextFieldEvent::KeyUp(_) => return,
            };
            this.log.push(line);
            if this.log.len() > 8 {
                this.log.remove(0);
            }
            cx.notify();
        });
        Self {
            field,
            popup: false,
            popup_focus: cx.focus_handle(),
            log: Vec::new(),
            _subscription: subscription,
        }
    }

    fn open_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = true;
        window.focus(&self.popup_focus, cx);
        cx.notify();
    }

    fn close_popup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = false;
        // The owner restores focus through plain focus: selection and scroll are preserved.
        let handle = self.field.focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }
}

impl Focusable for Example {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.popup_focus.clone()
    }
}

impl EventEmitter<()> for Example {}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .size_full()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xF0F1F3))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(24.0))
            .child(div().w(px(360.0)).child(self.field.clone()))
            .child(
                div()
                    .id("open-popup")
                    .w(px(120.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(4.0))
                    .bg(rgb(0x30343D))
                    .child("Open popup")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.open_popup(window, cx)),
                    ),
            );
        if self.popup {
            root = root.child(
                div()
                    .id("popup")
                    .track_focus(&self.popup_focus)
                    .on_key_down(cx.listener(
                        |this, event: &vespertine::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                this.close_popup(window, cx);
                            }
                        },
                    ))
                    .w(px(240.0))
                    .p(px(8.0))
                    .rounded(px(6.0))
                    .bg(rgb(0x30343D))
                    .child("Popup (Escape closes)")
                    .child(
                        div()
                            .id("close-popup")
                            .mt(px(6.0))
                            .child("Close")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| this.close_popup(window, cx)),
                            ),
                    ),
            );
        }
        root.children(self.log.iter().map(|line| div().child(line.clone())))
    }
}

fn run_script(cx: &mut App, window: WindowHandle<Example>) {
    cx.spawn(async move |cx| {
        let mut failures: Vec<String> = Vec::new();
        let mut check = |ok: bool, what: &str| {
            if !ok {
                failures.push(what.to_owned());
            }
        };
        let pause = |cx: &mut vespertine::AsyncApp| {
            cx.background_executor().timer(Duration::from_millis(80))
        };
        pause(cx).await;
        let key = |cx: &mut vespertine::AsyncApp, keystroke: &str| {
            let keystroke = Keystroke::parse(keystroke).expect("keystroke");
            let any: vespertine::AnyWindowHandle = window.into();
            cx.update_window(any, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
            .ok();
        };
        let focus = |cx: &mut vespertine::AsyncApp| {
            window
                .update(cx, |example, window, cx| {
                    let handle = example.field.focus_handle(cx);
                    window.focus(&handle, cx)
                })
                .ok();
        };
        focus(cx);
        pause(cx).await;
        key(cx, "enter");
        key(cx, "escape");
        let log = window
            .read_with(cx, |example, _| example.log.clone())
            .unwrap_or_default();
        check(
            log.iter().any(|line| line == "submitted: hello"),
            "Enter submits the exact value",
        );
        check(log.iter().any(|line| line == "cancelled"), "Escape cancels");
        key(cx, "ctrl-a");
        let selection = window
            .read_with(cx, |example, cx| example.field.read(cx).selected_range())
            .unwrap_or_default();
        check(selection == (0..5), "Ctrl+A selects all");
        // The system clipboard needs an input serial on Wayland, so the script pastes the item
        // through the same flow the Paste action uses.
        window
            .update(cx, |example, window, cx| {
                example.field.update(cx, |field, cx| {
                    field.paste_item(
                        &ClipboardItem::new_string("café ☕ 日本語".into()),
                        window,
                        cx,
                    )
                })
            })
            .ok();
        let text = window
            .read_with(cx, |example, cx| {
                example.field.read(cx).value_text().to_owned()
            })
            .unwrap_or_default();
        check(text == "café ☕ 日本語", "paste inserts non-ASCII text");
        window
            .update(cx, |example, window, cx| {
                example.field.update(cx, |field, cx| {
                    field.set_selected_range(1..5, false, window, cx)
                })
            })
            .ok();
        for _ in 0..3 {
            window
                .update(cx, |example, window, cx| example.open_popup(window, cx))
                .ok();
            pause(cx).await;
            window
                .update(cx, |example, window, cx| example.close_popup(window, cx))
                .ok();
            pause(cx).await;
        }
        let (text_after, selection_after, focused) = window
            .update(cx, |example, window, cx| {
                let field = example.field.read(cx);
                (
                    field.value_text().to_owned(),
                    field.selected_range(),
                    field.focus_handle(cx).is_focused(window),
                )
            })
            .unwrap_or_default();
        check(
            text_after == "café ☕ 日本語",
            "popup cycles leave the text unchanged",
        );
        check(
            selection_after == (1..5),
            "popup cycles leave the selection unchanged",
        );
        check(focused, "popup close restores focus to the field");
        for failure in &failures {
            eprintln!("FAILED: {failure}");
        }
        cx.update(|cx| cx.quit());
        std::process::exit(if failures.is_empty() { 0 } else { 1 });
    })
    .detach();
}

fn main() {
    let script = std::env::var_os("TEXT_FIELD_SCRIPT").is_some_and(|value| value == "1");
    application().run(move |cx: &mut App| {
        bind_default_keys(cx);
        let bounds = Bounds::centered(None, size(px(480.), px(320.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Example::new(window, cx)),
            )
            .expect("open the window");
        if script {
            run_script(cx, window);
        }
        cx.activate(true);
    });
}
