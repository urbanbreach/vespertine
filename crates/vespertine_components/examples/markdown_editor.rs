//! CMP-009 A2: styling never removes editable delimiters or alters copied source.
//!
//! Interactively the window shows a Markdown editor: emphasis, strong text, strikethrough and
//! inline code lose their delimiters on lines the caret is not on, and the delimiters come back
//! when the caret enters the line. With `MARKDOWN_EDITOR_SCRIPT=1` the example runs the
//! acceptance script and exits 0 when every check passes (1 otherwise): for
//! `**bold** and ~~gone~~ and `code`` with the caret on another line the displayed text hides all
//! delimiters, select-all selects the exact original string for copying, and moving the caret onto the
//! line shows every delimiter again.

use vespertine::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Entity, ParentElement, Render, Styled,
    WindowBounds, WindowHandle, WindowOptions, div, px, rgb, size,
};
use vespertine::{Context, IntoElement, Keystroke, Window};
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

async fn script(cx: &mut AsyncApp, window: WindowHandle<Example>) -> Vec<String> {
    let mut failures = Vec::new();
    let pause = |cx: &AsyncApp| {
        cx.background_executor()
            .timer(std::time::Duration::from_millis(120))
    };
    pause(cx).await;
    let any: AnyWindowHandle = window.into();
    let area = window
        .read_with(cx, |example, _| example.area.clone())
        .expect("example state");
    let read = |cx: &mut AsyncApp| {
        let area = area.clone();
        cx.update(|cx| {
            let area = area.read(cx);
            (area.display_text().to_owned(), area.source().to_owned())
        })
    };
    let select = |cx: &mut AsyncApp, at: usize| {
        let area = area.clone();
        cx.update_window(any, |_, window, cx| {
            area.update(cx, |area, cx| {
                area.focus(window, cx);
                area.set_selected_range(at..at, false, window, cx)
            })
        })
        .ok();
    };
    // The caret is on the second line: every delimiter of the first line is hidden.
    select(cx, SOURCE.len());
    pause(cx).await;
    let (display, source) = read(cx);
    if display != "bold and gone and code\nother line" {
        failures.push(format!("the display text is {display:?}"));
    }
    if source != SOURCE {
        failures.push(format!("the source changed to {source:?}"));
    }
    // Select everything: copy writes the selected source text, which is the exact source.
    cx.update_window(any, |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("ctrl-a").expect("keystroke"), cx);
    })
    .ok();
    let selected = cx.update(|cx| area.read(cx).core().selected_text().to_owned());
    if selected != SOURCE {
        failures.push(format!("the copied text would be {selected:?}"));
    }
    // The caret on the line shows every delimiter again.
    select(cx, 3);
    pause(cx).await;
    let (display, _) = read(cx);
    if display != SOURCE {
        failures.push(format!("the active line shows {display:?}"));
    }
    failures
}

fn main() {
    let run_script = std::env::var_os("MARKDOWN_EDITOR_SCRIPT").is_some_and(|value| value == "1");
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
        if run_script {
            cx.spawn(async move |cx| {
                let failures = script(cx, window).await;
                for failure in &failures {
                    eprintln!("FAILED: {failure}");
                }
                cx.update(|cx| cx.quit());
                std::process::exit(if failures.is_empty() { 0 } else { 1 });
            })
            .detach();
        } else {
            window
                .update(cx, |example, window, cx| {
                    example.area.update(cx, |area, cx| area.focus(window, cx))
                })
                .ok();
        }
        cx.activate(true);
    });
}
