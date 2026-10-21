//! CMP-008 A1: the multiline editor composing non-Latin text.
//!
//! The window shows an auto-growing editor (1 to 10 rows, then it scrolls). The composition
//! acceptance checks (marked text "に" and "日本" committed as "日本語", one undo restoring the
//! pre-composition source and selection) run in `tests/text_area.rs`.

use vespertine::{
    App, AppContext, Bounds, Entity, ParentElement, Render, Styled, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use vespertine::{Context, IntoElement, Window};
use vespertine_components::text::{TextArea, actions::bind_default_keys};
use vespertine_platform::application;

struct Example {
    area: Entity<TextArea>,
}

impl Render for Example {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xF0F1F3))
            .p(px(24.0))
            .child(div().w(px(420.0)).child(self.area.clone()))
    }
}

fn main() {
    application().run(move |cx: &mut App| {
        bind_default_keys(cx);
        let bounds = Bounds::centered(None, size(px(480.), px(360.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let area = cx.new(|cx| {
                        TextArea::new(window, cx)
                            .placeholder("Write something")
                            .value("hello\nworld")
                    });
                    cx.new(|_| Example { area })
                },
            )
            .expect("open the window");
        window
            .update(cx, |example, window, cx| {
                example.area.update(cx, |area, cx| area.focus(window, cx))
            })
            .ok();
        cx.activate(true);
    });
}
