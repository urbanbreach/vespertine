//! CMP-011 A1: Markdown streamed in pieces that end in the middle of syntax (an unclosed strong
//! run, an unfinished link, a half-streamed formula and an unclosed fence).
//!
//! Every frame appends the next piece. The example checks, at each step, that the stored source is
//! exactly the appended text; the canonical-tree, stable-prefix and repair checks are in the
//! parser tests. At the end it checks the whole-document copy and the exact text of a selected
//! code block, prints `ok` and exits when `MARKDOWN_STREAM_EXIT=1`; otherwise it keeps the
//! finished document on screen.

use std::time::Duration;
use vespertine::{
    App, AppContext, Bounds, Context, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, px, rgb, size,
};
use vespertine_components::markdown_doc::{BlockKind, Markdown, MarkdownDocument};
use vespertine_platform::application;
use vespertine_richtext::{TextService, text_root};

const PIECES: &[&str] = &[
    "# Streaming\n\nSome **bold text that is not closed",
    " yet** and a [link](https://exam",
    "ple.com/page) to follow, plus $x^",
    "2$ and a display $$a + b $",
    "$ formula.\n\n- item one\n- item **two",
    "**\n\n```rust\nlet answer = 42;",
    "\n\nfn main() {}\n```\n",
];

struct Stream {
    document: MarkdownDocument,
    fed: usize,
}

impl Stream {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(350))
                    .await;
                let Ok(done) = this.update_in(cx, |this, window, cx| this.step(window, cx)) else {
                    return;
                };
                if done {
                    break;
                }
            }
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            this.update_in(cx, |this, window, cx| this.finish(window, cx))
                .ok();
        })
        .detach();
        Self {
            document: MarkdownDocument::new(),
            fed: 0,
        }
    }

    fn step(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let piece = PIECES[self.fed];
        self.document.append(piece);
        self.fed += 1;
        let expected: String = PIECES[..self.fed].concat();
        assert_eq!(
            self.document.source(),
            expected,
            "the stored source is exactly the appended text"
        );
        cx.notify();
        self.fed == PIECES.len()
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source: String = PIECES.concat();
        assert_eq!(
            self.document.source(),
            source,
            "whole-document copy is the exact source"
        );
        assert!(matches!(
            self.document.tree().last().map(|b| &b.kind),
            Some(BlockKind::Code { code, .. }) if code == "let answer = 42;\n\nfn main() {}"
        ));
        let service = TextService::of(window, cx);
        println!(
            "source bytes: {}; selection: {:?}",
            source.len(),
            service.selected_text()
        );
        println!("ok");
        if std::env::var_os("MARKDOWN_STREAM_EXIT").is_some_and(|v| v == "1") {
            cx.quit();
        }
    }
}

impl Render for Stream {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        text_root(
            div()
                .size_full()
                .bg(rgb(0x181818))
                .text_color(rgb(0xeeeeee))
                .p(px(20.))
                .child(
                    Markdown::document("md", self.document.clone())
                        .streaming(self.fed < PIECES.len()),
                ),
        )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(640.), px(480.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Stream::new(window, cx)),
        )
        .expect("open window");
        cx.activate(true);
    });
}
