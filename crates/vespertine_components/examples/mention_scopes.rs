//! CMP-010 A2: copying between two attachment scopes.
//!
//! The window shows two editors, each owning an attachment scope (`scope-a` and `scope-b`), with
//! `⟦file:path⟧` and `⟦img:N:label⟧` references shown as chips. With `MENTION_SCOPES_SCRIPT=1`
//! the example runs the acceptance script and exits 0 when every check passes (1 otherwise):
//! within the same scope the pasted source restores the chips; into a different scope a source
//! that references attachment numbers pastes as plain expanded text; a source without attachment
//! references (file mentions) pastes as chips in either scope; and replacing the clipboard text
//! afterwards makes the structured metadata ignored.

use std::{collections::BTreeMap, sync::Arc};
use vespertine::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, ClipboardItem, Context, Entity,
    IntoElement, ParentElement, Render, Styled, Window, WindowBounds, WindowHandle, WindowOptions,
    div, px, rgb, size,
};
use vespertine_components::{
    mentions::{ClosureGrammar, ReferenceDescriptor, ReferenceGrammar, ReferenceKind},
    text::{TextArea, actions::bind_default_keys, editor::EditorHost},
};
use vespertine_platform::application;

const OPEN: char = '⟦';
const CLOSE: char = '⟧';

/// `⟦file:path⟧` is a file mention, `⟦img:N:label⟧` image attachment `N`.
fn grammar() -> Arc<dyn ReferenceGrammar> {
    Arc::new(ClosureGrammar::new(|source| {
        let mut found = Vec::new();
        let mut search = 0;
        while let Some(open) = source[search..].find(OPEN) {
            let start = search + open;
            let Some(close) = source[start..].find(CLOSE) else {
                break;
            };
            let end = start + close + CLOSE.len_utf8();
            let body = &source[start + OPEN.len_utf8()..end - CLOSE.len_utf8()];
            let parts: Vec<&str> = body.splitn(3, ':').collect();
            let descriptor = |kind, name: &str, identity: &str, attachment| ReferenceDescriptor {
                range: start..end,
                kind,
                name: name.to_owned(),
                identity: identity.to_owned(),
                attachment,
            };
            match parts.as_slice() {
                ["file", path] => found.push(descriptor(
                    ReferenceKind::File,
                    path.rsplit('/').next().unwrap_or(path),
                    path,
                    None,
                )),
                ["img", number, label] => found.push(descriptor(
                    ReferenceKind::Image,
                    label,
                    label,
                    number.parse().ok(),
                )),
                _ => {}
            }
            search = end;
        }
        found
    }))
}

struct Example {
    first: Entity<TextArea>,
    second: Entity<TextArea>,
}

impl Render for Example {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xF0F1F3))
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(div().w(px(520.0)).child(self.first.clone()))
            .child(div().w(px(520.0)).child(self.second.clone()))
    }
}

fn editor(window: &mut Window, cx: &mut Context<TextArea>, scope: &str, value: &str) -> TextArea {
    let mut area = TextArea::new(window, cx).with_references(vec![grammar()]);
    let mut live = BTreeMap::new();
    if scope == "scope-a" {
        live.insert(1, None);
    }
    area.set_live_attachments(scope, live, cx);
    area.set_value(value, window, cx);
    area
}

const SOURCE: &str = "look ⟦img:1:Image 1.png⟧ and ⟦file:src/a.rs⟧";
const EXPANDED: &str = "look Image 1.png and src/a.rs";

async fn script(cx: &mut AsyncApp, window: WindowHandle<Example>) -> Vec<String> {
    let mut failures = Vec::new();
    cx.background_executor()
        .timer(std::time::Duration::from_millis(120))
        .await;
    let any: AnyWindowHandle = window.into();
    let (first, second) = window
        .read_with(cx, |example, _| {
            (example.first.clone(), example.second.clone())
        })
        .expect("example state");
    let with = |cx: &mut AsyncApp,
                area: &Entity<TextArea>,
                f: &dyn Fn(
        &mut TextArea,
        &mut Window,
        &mut Context<TextArea>,
    ) -> Option<ClipboardItem>| {
        let area = area.clone();
        cx.update_window(any, |_, window, cx| {
            area.update(cx, |area, cx| f(area, window, cx))
        })
        .ok()
        .flatten()
    };
    let state = |cx: &mut AsyncApp, area: &Entity<TextArea>| {
        let area = area.clone();
        cx.update(|cx| {
            let area = area.read(cx);
            (area.source().to_owned(), area.chips().len())
        })
    };

    // Copy everything from the first editor.
    let item = with(cx, &first, &|area, window, cx| {
        area.select_all(window, cx);
        area.copy_item(cx)
    })
    .expect("a copy item");
    if item.text().as_deref() != Some(EXPANDED) {
        failures.push(format!("the clipboard text is {:?}", item.text()));
    }
    // Within the same scope the pasted source restores the chips.
    let paste = |item: ClipboardItem| {
        move |area: &mut TextArea, window: &mut Window, cx: &mut Context<TextArea>| {
            area.set_value("", window, cx);
            area.paste_item(&item, window, cx);
            None
        }
    };
    with(cx, &first, &paste(item.clone()));
    let (text, chips) = state(cx, &first);
    if text != SOURCE || chips != 2 {
        failures.push(format!("same scope pasted {text:?} with {chips} chips"));
    }
    // Into a different scope the source naming an attachment pastes as plain expanded text.
    with(cx, &second, &paste(item));
    let (text, chips) = state(cx, &second);
    if text != EXPANDED || chips != 0 {
        failures.push(format!("foreign scope pasted {text:?} with {chips} chips"));
    }
    // A source without attachment references pastes as chips in either scope.
    let files = with(cx, &first, &|area, window, cx| {
        area.set_value("see ⟦file:src/a.rs⟧", window, cx);
        area.select_all(window, cx);
        area.copy_item(cx)
    })
    .expect("a copy item");
    with(cx, &second, &paste(files.clone()));
    let (text, chips) = state(cx, &second);
    if text != "see ⟦file:src/a.rs⟧" || chips != 1 {
        failures.push(format!("a file mention pasted {text:?} with {chips} chips"));
    }
    // Replacing the clipboard text afterwards makes the metadata ignored.
    let metadata = files.entries().iter().find_map(|entry| match entry {
        vespertine::ClipboardEntry::String(string) => string.metadata.clone(),
        _ => None,
    });
    let stale = ClipboardItem::new_string_with_metadata(
        "edited meanwhile".to_owned(),
        metadata.expect("the copy carries metadata"),
    );
    with(cx, &second, &paste(stale));
    let (text, chips) = state(cx, &second);
    if text != "edited meanwhile" || chips != 0 {
        failures.push(format!("stale metadata pasted {text:?} with {chips} chips"));
    }
    failures
}

fn main() {
    let run_script = std::env::var_os("MENTION_SCOPES_SCRIPT").is_some_and(|value| value == "1");
    application().run(move |cx: &mut App| {
        bind_default_keys(cx);
        let bounds = Bounds::centered(None, size(px(600.), px(320.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |window, cx| {
                    let first = cx.new(|cx| editor(window, cx, "scope-a", SOURCE));
                    let second = cx.new(|cx| editor(window, cx, "scope-b", ""));
                    cx.new(|_| Example { first, second })
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
                    example.first.update(cx, |area, cx| area.focus(window, cx))
                })
                .ok();
        }
        cx.activate(true);
    });
}
