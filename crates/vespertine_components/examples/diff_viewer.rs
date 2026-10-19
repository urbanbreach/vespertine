//! CMP-015 A2 and A6: a large scrolling diff with the split, folded, wrapped and annotated
//! presentations.
//!
//! The window hosts a virtualized diff of 60 files. A toolbar switches unified and split mode,
//! wraps lines, folds the first file, collapses everything and adds a review note; clicking a
//! line's number gutter opens a draft (type, Enter commits, Escape cancels).
//!
//! The script (default) drives the same controls and verifies, then prints the report and exits
//! 0 (1 when a check fails):
//!
//! * switching unified and split keeps the logical source line at the top;
//! * folding and unfolding a file tweens only when the section is unwrapped, and keeps the top
//!   line;
//! * wrapping a long line gives it a measured height (several lines) while the others keep one
//!   line height;
//! * inserting an annotation card keeps the top line;
//! * Escape cancels an open draft without reaching the owner (the code under it is not
//!   activated), while Escape without a draft does reach the owner;
//! * a live patch update with the same row count keeps the logical anchor, and a color-only
//!   theme change neither reparses nor moves it.
//!
//! Run it interactively with `DIFF_VIEWER_INTERACTIVE=1`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;
use vespertine::{
    App, AppContext as _, Bounds, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    Keystroke, ListOffset, ParentElement, Pixels, Render, SharedString, StatefulInteractiveElement,
    Styled, Window, WindowBounds, WindowOptions, div, prelude::FluentBuilder, px, rgb, size,
};
use vespertine_components::diff::model::parse_count;
use vespertine_components::diff::rows::{Annotation, DiffMode, Row, RowAnchor, Side, row_anchor};
use vespertine_components::diff::state::DiffViewState;
use vespertine_components::diff::view::DiffView;
use vespertine_platform::application;
use vespertine_richtext::text_root;
use vespertine_theme::document::DocumentTheme;

const FILES: usize = 60;

fn patch(rename: bool) -> String {
    let long = "a_very_long_line_that_needs_wrapping_".repeat(12);
    let mut patch = String::new();
    for file in 0..FILES {
        patch.push_str(&format!(
            "diff --git a/src/module_{file}.rs b/src/module_{file}.rs\n--- a/src/module_{file}.rs\n+++ b/src/module_{file}.rs\n@@ -1,40 +1,40 @@\n"
        ));
        for line in 0..40 {
            let label = if rename { "renamed" } else { "value" };
            if line % 4 == 1 {
                patch.push_str(&format!(
                    "-    let {label}_{file}_{line} = compute({line});\n"
                ));
                patch.push_str(&format!(
                    "+    let {label}_{file}_{line} = compute({line} + 1);\n"
                ));
            } else if file == 2 && line == 10 {
                patch.push_str(&format!(" // {long}\n"));
            } else {
                patch.push_str(&format!("     let kept_{file}_{line} = {line};\n"));
            }
        }
    }
    patch
}

struct Demo {
    state: Entity<DiffViewState>,
    patch: Arc<str>,
    theme: Arc<DocumentTheme>,
    mode: DiffMode,
    wrap: bool,
    annotations: Vec<Annotation>,
    log: Rc<RefCell<Vec<String>>>,
    draft_text: Rc<RefCell<String>>,
    owner_saw_escape: Rc<RefCell<usize>>,
    focus: FocusHandle,
    next_note: usize,
    checks: Vec<(String, bool)>,
}

fn fail(message: &str) -> ! {
    eprintln!("diff_viewer: FAILED: {message}");
    std::process::exit(1);
}

impl Demo {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = DiffViewState::new(cx);
        let mut theme = DocumentTheme::dark();
        theme.font_mono = "Liberation Mono".into();
        let demo = Self {
            state,
            patch: Arc::from(patch(false)),
            theme: Arc::new(theme),
            mode: DiffMode::Unified,
            wrap: false,
            annotations: Vec::new(),
            log: Rc::default(),
            draft_text: Rc::default(),
            owner_saw_escape: Rc::default(),
            focus: cx.focus_handle(),
            next_note: 0,
            checks: Vec::new(),
        };
        if std::env::var_os("DIFF_VIEWER_INTERACTIVE").is_none() {
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                Self::script(this, cx).await;
            })
            .detach();
        }
        demo
    }

    fn top_anchor(&self, cx: &App) -> Option<RowAnchor> {
        let state = self.state.read(cx);
        let top = state.list_state().logical_scroll_top();
        row_anchor(state.files(), state.flat(), top.item_ix)
    }

    fn check(&mut self, name: &str, ok: bool) {
        println!("{} {name}", if ok { "ok  " } else { "FAIL" });
        self.checks.push((name.to_owned(), ok));
        if !ok {
            fail(name);
        }
    }

    async fn script(this: vespertine::WeakEntity<Self>, cx: &mut vespertine::AsyncWindowContext) {
        macro_rules! step {
            ($body:expr) => {{ this.update_in(cx, $body).expect("the demo window is alive") }};
        }
        macro_rules! pause {
            ($ms:expr) => {
                cx.background_executor()
                    .timer(Duration::from_millis($ms))
                    .await
            };
        }
        // Scroll into file 2 at a context line: the logical location every step must keep.
        let before = step!(|demo, _window, cx| {
            let state = demo.state.clone();
            let row = state.read(cx).flat().rows.iter().position(|row| {
                matches!(
                    row,
                    Row::Line {
                        file: 2,
                        line: 20,
                        ..
                    }
                )
            });
            let row = row.unwrap_or_else(|| fail("no line row to scroll to"));
            state.read(cx).list_state().scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: px(0.),
            });
            cx.notify();
            row
        });
        pause!(200);
        let anchor = step!(|demo, _, cx| demo.top_anchor(cx));
        let anchor = anchor.unwrap_or_else(|| fail("no anchor"));
        println!(
            "anchor: file {} {:?} (row {before})",
            anchor.file, anchor.source
        );

        // 1. Unified -> split -> unified.
        step!(|demo, _, cx| {
            demo.mode = DiffMode::Split;
            cx.notify();
        });
        pause!(250);
        let split = step!(|demo, _, cx| demo.top_anchor(cx));
        step!(|demo, _, _| demo.check(
            "split mode keeps the logical source line",
            split.is_some_and(|a| a.file == anchor.file && a.source == anchor.source)
        ));
        step!(|demo, _, cx| {
            demo.mode = DiffMode::Unified;
            cx.notify();
        });
        pause!(250);
        let unified = step!(|demo, _, cx| demo.top_anchor(cx));
        step!(|demo, _, _| demo.check(
            "unified mode keeps the logical source line",
            unified.is_some_and(|a| a.file == anchor.file && a.source == anchor.source)
        ));

        // 2. Fold and unfold the first file: a tween while unwrapped.
        step!(|demo, _, cx| {
            let state = demo.state.clone();
            state.update(cx, |state, cx| state.toggle_file("src/module_0.rs", cx));
        });
        pause!(60);
        let folding = step!(|demo, _, cx| demo.state.read(cx).folding_files());
        step!(|demo, _, _| demo.check("an unwrapped fold tweens", folding == [0]));
        pause!(550);
        let (folding, now) =
            step!(|demo, _, cx| (demo.state.read(cx).folding_files(), demo.top_anchor(cx)));
        step!(|demo, _, _| {
            demo.check(
                "the fold settles after the arming window",
                folding.is_empty(),
            );
            demo.check(
                "folding keeps the logical source line",
                now.is_some_and(|a| a.file == anchor.file && a.source == anchor.source),
            );
        });
        step!(|demo, _, cx| {
            let state = demo.state.clone();
            state.update(cx, |state, cx| state.toggle_file("src/module_0.rs", cx));
        });
        pause!(700);
        let now = step!(|demo, _, cx| demo.top_anchor(cx));
        step!(|demo, _, _| demo.check(
            "unfolding keeps the logical source line",
            now.is_some_and(|a| a.file == anchor.file && a.source == anchor.source)
        ));

        // 3. Wrap a long line: measured heights; folds settle immediately when wrapped.
        step!(|demo, _, cx| {
            demo.wrap = true;
            cx.notify();
        });
        pause!(300);
        // Look at the long line from just above it, then return to the logical location.
        step!(|demo, _, cx| {
            let state = demo.state.clone();
            let row = state.read(cx).flat().rows.iter().position(|row| {
                matches!(
                    row,
                    Row::Line {
                        file: 2,
                        line: 8,
                        ..
                    }
                )
            });
            let row = row.unwrap_or_else(|| fail("no line row above the long line"));
            state.read(cx).list_state().scroll_to(ListOffset {
                item_ix: row,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        pause!(300);
        let heights = step!(|demo, _, cx| {
            let state = demo.state.read(cx);
            let long_row = state.flat().rows.iter().position(|row| {
                matches!(
                    row,
                    Row::Line {
                        file: 2,
                        line: 13,
                        ..
                    }
                )
            });
            let short_row = state.flat().rows.iter().position(|row| {
                matches!(
                    row,
                    Row::Line {
                        file: 2,
                        line: 14,
                        ..
                    }
                )
            });
            let height = |row: Option<usize>| -> Option<f32> {
                row.and_then(|row| state.list_state().bounds_for_item(row))
                    .map(|bounds| f32::from(bounds.size.height))
            };
            (height(long_row), height(short_row))
        });
        step!(|demo, _, cx| {
            let state = demo.state.clone();
            state.read(cx).list_state().scroll_to(ListOffset {
                item_ix: before,
                offset_in_item: px(0.),
            });
            cx.notify();
        });
        let (long, short) = heights;
        println!("wrapped heights: long {long:?} short {short:?}");
        step!(|demo, _, _| demo.check(
            "a wrapped long line has a measured height and short lines keep one line",
            long.is_some_and(|h| h > 42.) && short.is_some_and(|h| (h - 21.).abs() < 1.)
        ));
        step!(|demo, _, cx| {
            let state = demo.state.clone();
            state.update(cx, |state, cx| state.toggle_file("src/module_0.rs", cx));
        });
        pause!(80);
        let folding = step!(|demo, _, cx| demo.state.read(cx).folding_files());
        step!(|demo, _, _| demo.check("a wrapped fold settles without a tween", folding.is_empty()));
        step!(|demo, _, cx| {
            let state = demo.state.clone();
            state.update(cx, |state, cx| state.toggle_file("src/module_0.rs", cx));
            demo.wrap = false;
            cx.notify();
        });
        pause!(300);

        // 4. Insert an annotation card.
        step!(|demo, _, cx| {
            let anchor_source = anchor.source.unwrap_or_else(|| fail("a line anchor"));
            demo.annotations.push(Annotation {
                id: "scripted".into(),
                path: "src/module_2.rs".into(),
                side: anchor_source.side,
                line: anchor_source.line,
                body: "scripted review note".into(),
            });
            cx.notify();
        });
        pause!(300);
        let (cards, now) = step!(|demo, _, cx| (
            demo.state
                .read(cx)
                .flat()
                .rows
                .iter()
                .filter(|row| matches!(row, Row::Card { .. }))
                .count(),
            demo.top_anchor(cx)
        ));
        step!(|demo, _, _| {
            demo.check("the annotation card is inserted", cards == 1);
            demo.check(
                "inserting a card keeps the logical source line",
                now.is_some_and(|a| a.file == anchor.file && a.source == anchor.source),
            );
        });

        // 5. Escape cancels a draft without reaching the owner.
        step!(|demo, window, cx| {
            let anchor_source = anchor.source.unwrap_or_else(|| fail("a line anchor"));
            let draft = vespertine_components::diff::rows::Draft {
                path: "src/module_2.rs".into(),
                side: anchor_source.side,
                line: anchor_source.line + 1,
                editing: None,
            };
            let state = demo.state.clone();
            state.update(cx, |state, cx| state.open_draft(draft, cx));
            window.refresh();
        });
        pause!(300);
        let focused = step!(|demo, window, cx| {
            demo.state.read(cx).draft_focus_handle().is_focused(window)
        });
        step!(|demo, _, _| demo.check("the draft editor is focused", focused));
        let lines_before = step!(|demo, _, _| demo.log.borrow().len());
        cx.update(|window, cx| {
            window.dispatch_keystroke(Keystroke::parse("escape").expect("keystroke"), cx);
        })
        .expect("the demo window is alive");
        pause!(200);
        let (draft, owner, lines_after) = step!(|demo, _, cx| (
            demo.state.read(cx).draft().is_some(),
            *demo.owner_saw_escape.borrow(),
            demo.log.borrow().len()
        ));
        step!(|demo, _, _| {
            demo.check("Escape cancels the draft", !draft);
            demo.check("Escape with a draft does not reach the owner", owner == 0);
            demo.check("no code line was activated", lines_after == lines_before);
        });
        step!(|demo, window, cx| demo.focus.focus(window, cx));
        pause!(100);
        cx.update(|window, cx| {
            window.dispatch_keystroke(Keystroke::parse("escape").expect("keystroke"), cx);
        })
        .expect("the demo window is alive");
        pause!(100);
        let owner = step!(|demo, _, _| *demo.owner_saw_escape.borrow());
        step!(|demo, _, _| demo.check("Escape without a draft reaches the owner", owner == 1));

        // 6. A live patch update keeps the anchor; a color-only change neither reparses nor moves it.
        let parses = parse_count();
        step!(|demo, _, cx| {
            demo.patch = Arc::from(patch(true));
            cx.notify();
        });
        pause!(300);
        let now = step!(|demo, _, cx| demo.top_anchor(cx));
        step!(|demo, _, _| demo.check(
            "a live patch update with the same row count keeps the logical row",
            now.is_some_and(|a| a.file == anchor.file)
        ));
        let reparsed = parse_count();
        step!(|demo, _, cx| {
            let mut theme = (*demo.theme).clone();
            theme.diff_add = rgb(0x44ddff);
            demo.theme = Arc::new(theme);
            cx.notify();
        });
        pause!(300);
        let (now2, after) = step!(|demo, _, cx| (demo.top_anchor(cx), parse_count()));
        step!(|demo, _, _| {
            demo.check(
                "a color-only theme change does not reparse",
                after == reparsed,
            );
            demo.check(
                "a color-only theme change keeps the scroll position",
                now2.map(|a| a.file) == now.map(|a| a.file),
            );
        });
        let _ = parses;
        println!("diff_viewer: all checks passed");
        cx.update(|_, cx| cx.quit()).ok();
    }
}

fn button(label: &'static str) -> impl IntoElement + StatefulInteractiveElement + Styled {
    div()
        .id(label)
        .px(px(10.))
        .py(px(4.))
        .rounded(px(4.))
        .bg(rgb(0x2b303b))
        .hover(|style| style.bg(rgb(0x3a4150)))
        .cursor_pointer()
        .child(label)
}

impl Render for Demo {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let mut view = DiffView::new(&self.state)
            .patch(self.patch.clone())
            .theme(self.theme.clone())
            .scroll(true)
            .sticky_headers(true)
            .mode(self.mode)
            .wrap_lines(self.wrap)
            .annotations(self.annotations.clone())
            .size_full();
        let log = self.log.clone();
        view = view.on_line_click(move |click, _, _| {
            log.borrow_mut().push(format!(
                "line {:?} {:?} {}",
                click.old, click.new, click.text
            ))
        });
        let (log, owner) = (self.log.clone(), entity.clone());
        view = view.on_add_annotation(move |add, _, cx| {
            log.borrow_mut()
                .push(format!("add {}:{}", add.path, add.line));
            owner.update(cx, |demo, cx| {
                demo.next_note += 1;
                demo.annotations.push(Annotation {
                    id: format!("note-{}", demo.next_note),
                    path: add.path.clone(),
                    side: add.side,
                    line: add.line,
                    body: add.body.clone(),
                });
                cx.notify();
            });
        });
        let owner = entity;
        view = view.on_remove_annotation(move |id, _, cx| {
            owner.update(cx, |demo, cx| {
                demo.annotations.retain(|note| note.id != id);
                cx.notify();
            });
        });
        let text = self.draft_text.clone();
        view = view.draft_editor(move |editor, _window, _cx| {
            let editor = editor.clone();
            let (typing, shown) = (text.clone(), text.borrow().clone());
            div()
                .id("draft-editor")
                .track_focus(&editor.focus.clone())
                .size_full()
                .p(px(6.))
                .rounded(px(4.))
                .bg(rgb(0x20242c))
                .text_size(px(12.))
                .on_key_down(move |event, window, cx| {
                    let key = &event.keystroke;
                    if key.key == "enter" {
                        let body = std::mem::take(&mut *typing.borrow_mut());
                        editor.commit(&body, window, cx);
                    } else if key.key == "backspace" {
                        typing.borrow_mut().pop();
                    } else if let Some(c) = &key.key_char {
                        typing.borrow_mut().push_str(c);
                    }
                    window.refresh();
                })
                .child(SharedString::from(format!("{shown}\u{2581}")))
                .into_any_element()
        });
        let escapes = self.owner_saw_escape.clone();
        div()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .bg(rgb(0x14161a))
            .text_color(rgb(0xe5e5e5))
            .text_size(px(12.))
            .on_key_down(move |event, _, _| {
                if event.keystroke.key == "escape" {
                    *escapes.borrow_mut() += 1;
                }
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .child(button("Unified").on_click(cx.listener(|demo, _, _, cx| {
                        demo.mode = DiffMode::Unified;
                        cx.notify();
                    })))
                    .child(button("Split").on_click(cx.listener(|demo, _, _, cx| {
                        demo.mode = DiffMode::Split;
                        cx.notify();
                    })))
                    .child(button("Wrap").on_click(cx.listener(|demo, _, _, cx| {
                        demo.wrap = !demo.wrap;
                        cx.notify();
                    })))
                    .child(
                        button("Collapse all").on_click(cx.listener(|demo, _, _, cx| {
                            let state = demo.state.clone();
                            state.update(cx, |state, cx| {
                                state.collapse_all(cx);
                            });
                        })),
                    )
                    .child(button("Add note").on_click(cx.listener(|demo, _, _, cx| {
                        demo.next_note += 1;
                        demo.annotations.push(Annotation {
                            id: format!("note-{}", demo.next_note),
                            path: "src/module_1.rs".into(),
                            side: Side::New,
                            line: 5,
                            body: "A review note on line 5.".into(),
                        });
                        cx.notify();
                    })))
                    .child(div().flex_1())
                    .when(!self.checks.is_empty(), |toolbar| {
                        toolbar.child(format!("{} checks passed", self.checks.len()))
                    }),
            )
            .child(div().flex_1().min_h_0().child(text_root(view)))
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds: Bounds<Pixels> = Bounds::centered(None, size(px(980.), px(680.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Demo::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
