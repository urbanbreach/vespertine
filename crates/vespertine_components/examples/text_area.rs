//! CMP-008 A1: the multiline editor composing non-Latin text.
//!
//! Interactively the window shows an auto-growing editor (1 to 10 rows, then it scrolls). With
//! `TEXT_AREA_SCRIPT=1` the example runs the acceptance script and exits 0 when every check
//! passes (1 otherwise): marked text "に" and "日本" is committed as "日本語", one undo restores
//! exactly the pre-composition source and selection, and the same holds when a non-empty
//! selection is replaced.

use vespertine::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Entity, EntityInputHandler, ParentElement,
    Render, Styled, WindowBounds, WindowHandle, WindowOptions, div, px, rgb, size,
};
use vespertine::{Context, IntoElement, Keystroke, Window};
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

async fn script(cx: &mut AsyncApp, window: WindowHandle<Example>) -> Vec<String> {
    let mut failures = Vec::new();
    let pause = |cx: &AsyncApp| cx.background_executor().timer(std::time::Duration::from_millis(80));
    pause(cx).await;
    let any: AnyWindowHandle = window.into();
    let area = window
        .read_with(cx, |example, _| example.area.clone())
        .expect("example state");
    let run = |cx: &mut AsyncApp, f: &dyn Fn(&mut TextArea, &mut Window, &mut Context<TextArea>)| {
        let area = area.clone();
        cx.update_window(any, |_, window, cx| area.update(cx, |area, cx| f(area, window, cx)))
            .ok();
    };
    let read = |cx: &mut AsyncApp| {
        let area = area.clone();
        cx.update(|cx| {
            let area = area.read(cx);
            (area.source().to_owned(), area.selected_range())
        })
    };
    run(cx, &|area, window, cx| {
        area.set_value("abc", window, cx);
        area.focus(window, cx);
        area.set_selected_range(1..1, false, window, cx);
    });
    for marked in ["に", "日本"] {
        run(cx, &|area, window, cx| {
            area.replace_and_mark_text_in_range(None, marked, None, window, cx)
        });
    }
    run(cx, &|area, window, cx| area.replace_text_in_range(None, "日本語", window, cx));
    let (text, _) = read(cx);
    if text != "a日本語bc" {
        failures.push(format!("commit produced {text:?}"));
    }
    cx.update_window(any, |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("ctrl-z").expect("keystroke"), cx);
    })
    .ok();
    let (text, selection) = read(cx);
    if text != "abc" || selection != (1..1) {
        failures.push(format!("undo restored {text:?} {selection:?}"));
    }
    run(cx, &|area, window, cx| area.set_selected_range(1..2, false, window, cx));
    run(cx, &|area, window, cx| {
        area.replace_and_mark_text_in_range(None, "に", None, window, cx)
    });
    run(cx, &|area, window, cx| area.replace_text_in_range(None, "日本語", window, cx));
    cx.update_window(any, |_, window, cx| {
        window.dispatch_keystroke(Keystroke::parse("ctrl-z").expect("keystroke"), cx);
    })
    .ok();
    let (text, selection) = read(cx);
    if text != "abc" || selection != (1..2) {
        failures.push(format!("selection undo restored {text:?} {selection:?}"));
    }
    failures
}

fn main() {
    let run_script = std::env::var_os("TEXT_AREA_SCRIPT").is_some_and(|value| value == "1");
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
