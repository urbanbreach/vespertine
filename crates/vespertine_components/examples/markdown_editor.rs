//! CMP-009 A2: styling never removes editable delimiters or alters copied source.
//!
//! The window shows a Markdown editor: emphasis, strong text, strikethrough and inline code lose
//! their delimiters on lines the caret is not on, and the delimiters come back when the caret
//! enters the line. The acceptance checks (the displayed text hides every delimiter, select-all
//! copies the exact source) run in `tests/markdown_editor.rs`.

use vespertine::{
    App, AppContext, Bounds, Entity, ParentElement, Render, Styled, WindowBounds, WindowOptions,
    div, px, rgb, size,
};
use vespertine::{Context, IntoElement, Window};
use vespertine_components::{
    markdown::MarkdownOptions,
    text::{TextArea, actions::bind_default_keys},
};
use vespertine_platform::application;

const SOURCE: &str = "**bold** and ~~gone~~ and `code`\nother line";

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
                            .value(SOURCE)
                            .with_markdown(MarkdownOptions::default())
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
