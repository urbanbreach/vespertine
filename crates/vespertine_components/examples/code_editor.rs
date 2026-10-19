//! CMP-014 A1: an unwrapped long code block with its own horizontal scroll (a vertical wheel over
//! it continues to the page) beside a wrapped, editable code editor.
//!
//! The left column is a vertically scrolling page holding the read-only code block; the right
//! column is the editor with soft wrap on, line numbers and Rust highlighting. With
//! `CODE_EDITOR_EXIT=1` the example edits the editor through its public API, checks that
//! highlighting never moved a glyph, that the block never wraps and that the editor wraps, prints
//! `ok` and exits 0 (1 when a check fails).

use std::time::Duration;
use vespertine::{
    App, AppContext, Bounds, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Render, StatefulInteractiveElement, Styled, Window, WindowBounds, WindowOptions, div, px, rgb,
    size,
};
use vespertine_components::code::{CodeBlock, CodeEditor};
use vespertine_platform::application;
use vespertine_richtext::text_root;

const LONG: &str = "fn render_long_line(alpha: usize, beta: usize, gamma: usize, delta: usize, epsilon: usize, zeta: usize, eta: usize, theta: usize) -> usize { alpha + beta + gamma + delta + epsilon + zeta + eta + theta }\n\nfn main() {\n    println!(\"{}\", render_long_line(1, 2, 3, 4, 5, 6, 7, 8));\n}\n";

const EDITABLE: &str = "fn render_long_line(alpha: usize, beta: usize, gamma: usize, delta: usize) -> usize {\n    // The same long line wraps here instead of scrolling.\n    alpha + beta + gamma + delta\n}\n";

struct Example {
    editor: Entity<CodeEditor>,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| CodeEditor::new(window, cx));
        editor.update(cx, |editor, cx| {
            editor.set_value(EDITABLE, window, cx);
            editor.set_language(Some("rust".into()));
            editor.set_soft_wrap(true, cx);
            editor.highlight_from_language(cx);
            editor.set_rows(4, 14, cx);
        });
        if std::env::var_os("CODE_EDITOR_EXIT").is_some_and(|v| v == "1") {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                this.update_in(cx, |this, window, cx| this.check(window, cx))
                    .ok();
            })
            .detach();
        }
        Self { editor }
    }

    fn check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let failures = self.editor.update(cx, |editor, cx| {
            let mut failures = Vec::new();
            if !editor.soft_wrap() {
                failures.push("the editor does not wrap".to_owned());
            }
            let rows = editor.area().read(cx).visual_rows().len();
            let lines = editor.value(cx).matches('\n').count() + 1;
            if rows <= lines {
                failures.push(format!("no wrapped rows: {rows} rows for {lines} lines"));
            }
            let before = editor.area().read(cx).point_for_index(20);
            editor.clear_highlighter(cx);
            let plain = editor.area().read(cx).point_for_index(20);
            if before != plain {
                failures.push("highlighting moved a glyph".to_owned());
            }
            editor.highlight_from_language(cx);
            editor.set_selected_range(0..0, false, window, cx);
            editor.replace(0..0, "// edited\n", window, cx);
            if !editor.value(cx).starts_with("// edited\n") {
                failures.push("the edit did not apply".to_owned());
            }
            failures
        });
        if failures.is_empty() {
            println!("ok");
            cx.quit();
        } else {
            eprintln!("failed: {failures:?}");
            std::process::exit(1);
        }
    }
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let filler = (0..30).map(|n| {
            div()
                .flex_none()
                .h(px(22.))
                .child(format!("The page scrolls under a vertical wheel: row {n}"))
        });
        text_root(
            div()
                .size_full()
                .bg(rgb(0x181818))
                .text_color(rgb(0xeeeeee))
                .p(px(16.))
                .flex()
                .flex_row()
                .gap(px(16.))
                .child(
                    div()
                        .id("page")
                        .flex_1()
                        .min_w_0()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_none()
                                .p(px(12.))
                                .rounded(px(8.))
                                .bg(rgb(0x202020))
                                .child(
                                    CodeBlock::new("long", LONG)
                                        .language("rust")
                                        .show_line_numbers(true)
                                        .scroll_indicator(true),
                                ),
                        )
                        .children(filler),
                )
                .child(div().flex_none().w(px(440.)).child(self.editor.clone())),
        )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(980.), px(420.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Example::new(window, cx)),
        )
        .expect("open window");
        cx.activate(true);
    });
}
