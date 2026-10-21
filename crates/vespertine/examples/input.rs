#[path = "example_support/fonts.rs"]
mod example_support;
#[path = "example_support/text_input.rs"]
mod text_input;

use text_input::TextInput;
use vespertine::{
    App, Bounds, Context, Entity, FocusHandle, Focusable, KeyBinding, Keystroke, MouseButton,
    MouseUpEvent, Window, WindowBounds, WindowOptions, actions, black, div, opaque_grey,
    prelude::*, px, rgb, size, white, yellow,
};
use vespertine_platform::application;

actions!(text_input, [Quit]);

struct InputExample {
    text_input: Entity<TextInput>,
    recent_keystrokes: Vec<Keystroke>,
    focus_handle: FocusHandle,
}

impl Focusable for InputExample {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl InputExample {
    fn on_reset_click(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.recent_keystrokes.clear();
        self.text_input
            .update(cx, |text_input, cx| text_input.reset(window, cx));
        cx.notify();
    }
}

impl Render for InputExample {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .bg(rgb(0xaaaaaa))
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .bg(white())
                    .border_b_1()
                    .border_color(black())
                    .flex()
                    .flex_row()
                    .justify_between()
                    .child(format!("Keyboard {}", cx.keyboard_layout().name()))
                    .child(
                        div()
                            .border_1()
                            .border_color(black())
                            .px_2()
                            .bg(yellow())
                            .child("Reset")
                            .hover(|style| {
                                style
                                    .bg(yellow().blend(opaque_grey(0.5, 0.5)))
                                    .cursor_pointer()
                            })
                            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_reset_click)),
                    ),
            )
            .child(self.text_input.clone())
            .children(self.recent_keystrokes.iter().rev().map(|ks| {
                format!(
                    "{:} {}",
                    ks.unparse(),
                    if let Some(key_char) = ks.key_char.as_ref() {
                        format!("-> {:?}", key_char)
                    } else {
                        "".to_owned()
                    }
                )
            }))
    }
}

fn run_example() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(300.0), px(300.0)), cx);
        cx.bind_keys(text_input::key_bindings());

        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    let text_input = cx.new(|cx| TextInput::new("", "Type here...", cx));
                    cx.new(|cx| InputExample {
                        text_input,
                        recent_keystrokes: vec![],
                        focus_handle: cx.focus_handle(),
                    })
                },
            )
            .unwrap();
        let view = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        cx.observe_keystrokes(move |ev, _, cx| {
            view.update(cx, |view, cx| {
                view.recent_keystrokes.push(ev.keystroke.clone());
                cx.notify();
            })
        })
        .detach();
        cx.on_keyboard_layout_change({
            move |cx| {
                window.update(cx, |_, _, cx| cx.notify()).ok();
            }
        })
        .detach();

        window
            .update(cx, |view, window, cx| {
                window.focus(&view.text_input.focus_handle(cx), cx);
                cx.activate(true);
            })
            .unwrap();
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
    });
}

fn main() {
    run_example();
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use vespertine::{EntityInputHandler, TestAppContext};

    fn input(content: &'static str, cx: &mut Context<TextInput>) -> TextInput {
        TextInput::new(content, "", cx)
    }

    #[vespertine::test]
    fn native_selection_and_length_use_utf16(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| input("e🙂漢", cx));
        window
            .update(cx, |input, window, cx| {
                assert_eq!(input.text_length_utf16(window, cx), Some(4));
                input.set_selected_text_range(1..3, window, cx);
                assert_eq!(input.selected_range, 1..5);
                assert_eq!(
                    input.selected_text_range(false, window, cx).unwrap().range,
                    1..3
                );
                input.set_selected_text_range(3..99, window, cx);
                assert_eq!(input.selected_range, 5..8);
            })
            .unwrap();
    }

    #[vespertine::test]
    fn marked_selection_is_relative_to_inserted_text(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, cx| input("A🙂Z", cx));
        window
            .update(cx, |input, window, cx| {
                input.replace_and_mark_text_in_range(Some(1..3), "é🙂", Some(1..3), window, cx);
                assert_eq!(input.content, "Aé🙂Z");
                assert_eq!(input.marked_range, Some(1..7));
                assert_eq!(input.selected_range, 3..7);

                input.replace_and_mark_text_in_range(None, "漢", Some(1..1), window, cx);
                assert_eq!(input.content, "A漢Z");
                assert_eq!(input.marked_range, Some(1..4));
                assert_eq!(input.selected_range, 4..4);

                input.replace_and_mark_text_in_range(None, "", Some(0..0), window, cx);
                assert_eq!(input.content, "AZ");
                assert_eq!(input.marked_range, None);
                assert_eq!(input.selected_range, 1..1);
            })
            .unwrap();
    }

    #[vespertine::test]
    fn long_single_line_selection_uses_one_geometry_fragment(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| input("", cx));
        view.update(cx, |input, cx| {
            input.content = "a".repeat(10_000).into();
            cx.notify();
        });
        cx.run_until_parked();
        view.update_in(cx, |input, window, cx| {
            let bounds = input.last_bounds.expect("input was painted");
            let rectangles = input.selection_bounds_for_range(0..10_000, bounds, window, cx);
            assert_eq!(rectangles.len(), 1);
            assert!(rectangles[0].size.width > px(0.));
        });
    }
}
