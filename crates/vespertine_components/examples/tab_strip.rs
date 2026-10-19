//! CMP-017 A1: a tab strip over three small text editors.
//!
//! Each tab owns one editor (a focusable view with its own text, caret and focus state; the
//! simplest framework text input available, since the shared text field component is not part of
//! this crate). Drag a tab to reorder, click the close slot or middle-click to close, press
//! Ctrl+Tab / Ctrl+Shift+Tab to cycle, Ctrl+W to close the selected tab and click "+" for a new
//! tab. Surviving editors keep their text, caret and focus through every operation.
//!
//! With `TAB_STRIP_SCRIPT=1` the example runs the acceptance script with synthesized input and
//! exits 0 when every check passes (1 otherwise): the first tab is dragged over the third, an
//! inactive tab is closed (the selection stays), Ctrl+Tab cycles with wrap, the selected tab is
//! closed (the policy's survivor is selected) and every surviving editor keeps its state.

use std::time::Duration;
use vespertine::{
    App, AppContext, AsyncApp, Bounds, Context, Entity, EventEmitter, FocusHandle,
    InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, PlatformInput,
    Render, SharedString, Styled, WeakEntity, Window, WindowBounds, WindowHandle, WindowOptions,
    actions, div, point, prelude::FluentBuilder, px, size,
};
use vespertine_components::{
    nav_theme::NavTheme,
    tabs::{TabId, TabSpec, TabStrip, bind_keys, cycle_on},
};
use vespertine_platform::application;

actions!(tab_strip_example, [CloseSelected]);

/// Emitted when an editor's text changed.
struct Edited;

struct Editor {
    text: String,
    caret: usize,
    initial: String,
    focus: FocusHandle,
}

impl EventEmitter<Edited> for Editor {}

impl Editor {
    fn new(text: &str, caret: usize, cx: &mut Context<Self>) -> Self {
        Self {
            text: text.to_owned(),
            caret,
            initial: text.to_owned(),
            focus: cx.focus_handle(),
        }
    }

    fn is_dirty(&self) -> bool {
        self.text != self.initial
    }

    fn byte_index(&self, chars: usize) -> usize {
        self.text
            .char_indices()
            .nth(chars)
            .map_or(self.text.len(), |(ix, _)| ix)
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.alt || keystroke.modifiers.platform {
            return;
        }
        let length = self.text.chars().count();
        match keystroke.key.as_str() {
            "left" => self.caret = self.caret.saturating_sub(1),
            "right" => self.caret = (self.caret + 1).min(length),
            "home" => self.caret = 0,
            "end" => self.caret = length,
            "backspace" if self.caret > 0 => {
                let end = self.byte_index(self.caret);
                let start = self.byte_index(self.caret - 1);
                self.text.replace_range(start..end, "");
                self.caret -= 1;
                cx.emit(Edited);
            }
            "delete" if self.caret < length => {
                let start = self.byte_index(self.caret);
                let end = self.byte_index(self.caret + 1);
                self.text.replace_range(start..end, "");
                cx.emit(Edited);
            }
            _ => {
                let Some(typed) = keystroke.key_char.as_deref() else {
                    return;
                };
                let at = self.byte_index(self.caret);
                self.text.insert_str(at, typed);
                self.caret += typed.chars().count();
                cx.emit(Edited);
            }
        }
        cx.notify();
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = NavTheme::of(cx);
        let focused = self.focus.is_focused(window);
        let at = self.byte_index(self.caret);
        let (before, after) = self.text.split_at(at);
        div()
            .id("editor")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(160.))
            .p(px(12.))
            .rounded(px(6.))
            .border_1()
            .border_color(if focused {
                theme.accent
            } else {
                theme.popup_border
            })
            .bg(theme.popup_surface)
            .text_size(px(15.))
            .text_color(theme.text)
            .child(before.to_owned())
            .child(
                div()
                    .w(px(1.5))
                    .h(px(18.))
                    .bg(theme.accent)
                    .opacity(if focused { 1.0 } else { 0.3 }),
            )
            .child(after.to_owned())
    }
}

struct Example {
    strip: Entity<TabStrip>,
    editors: Vec<(TabId, Entity<Editor>)>,
    next_untitled: usize,
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let me: WeakEntity<Self> = cx.entity().downgrade();
        let strip = cx.new(|_| {
            let closed = me.clone();
            let added = me.clone();
            TabStrip::new("example")
                .theme(NavTheme::neutral())
                .on_close(move |id, _, cx| {
                    let id = id.clone();
                    closed
                        .update(cx, |this, cx| {
                            this.editors.retain(|(editor_id, _)| *editor_id != id);
                            cx.notify();
                        })
                        .ok();
                })
                .on_add(move |window, cx| {
                    added.update(cx, |this, cx| this.add_tab(window, cx)).ok();
                })
        });
        let mut example = Self {
            strip,
            editors: Vec::new(),
            next_untitled: 1,
        };
        for (id, title, text, caret) in [
            ("notes", "Notes", "alpha: the first editor", 5),
            ("todo", "Todo", "bravo second editor", 3),
            ("log", "Log", "charlie third editor", 7),
        ] {
            example.add_editor(TabId::from(id), title, text, caret, window, cx);
        }
        example.strip.update(cx, |strip, cx| {
            strip.select(&TabId::from("todo"), cx);
        });
        cx.observe(&example.strip, |_, _, cx| cx.notify()).detach();
        example.focus_selected(window, cx);
        example
    }

    fn add_editor(
        &mut self,
        id: TabId,
        title: &str,
        text: &str,
        caret: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = cx.new(|cx| Editor::new(text, caret, cx));
        let focus = editor.read(cx).focus.clone();
        let tab = id.clone();
        cx.subscribe(&editor, move |this, editor, _: &Edited, cx| {
            this.sync_dirty(&tab, editor.read(cx).is_dirty(), cx);
        })
        .detach();
        self.editors.push((id.clone(), editor));
        self.strip.update(cx, |strip, cx| {
            strip.set_content_focus(id.clone(), focus);
            let mut tabs = strip.specs().to_vec();
            tabs.push(TabSpec::new(id, SharedString::from(title.to_owned())));
            strip.set_tabs(tabs, cx);
        });
        let _ = window;
    }

    fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.next_untitled;
        self.next_untitled += 1;
        let id = TabId::from(format!("untitled-{n}"));
        self.add_editor(id.clone(), &format!("Untitled {n}"), "", 0, window, cx);
        self.strip.update(cx, |strip, cx| strip.select(&id, cx));
        self.focus_selected(window, cx);
    }

    fn sync_dirty(&mut self, id: &TabId, dirty: bool, cx: &mut Context<Self>) {
        self.strip.update(cx, |strip, cx| {
            let mut tabs = strip.specs().to_vec();
            if let Some(tab) = tabs.iter_mut().find(|tab| &tab.id == id) {
                tab.dirty = dirty;
            }
            strip.set_tabs(tabs, cx);
        });
    }

    fn focus_selected(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected) = self.strip.read(cx).selected_id().cloned() else {
            return;
        };
        if let Some((_, editor)) = self.editors.iter().find(|(id, _)| *id == selected) {
            let focus = editor.read(cx).focus.clone();
            window.focus(&focus, cx);
        }
    }

    fn close_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected) = self.strip.read(cx).selected_id().cloned() else {
            return;
        };
        self.strip.update(cx, |strip, cx| {
            strip.close_tab(&selected, window, cx);
        });
    }
}

impl Render for Example {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = NavTheme::neutral();
        let selected = self.strip.read(cx).selected_id().cloned();
        let shown = self
            .editors
            .iter()
            .find(|(id, _)| Some(id) == selected.as_ref())
            .map(|(_, editor)| editor.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(12.))
            .bg(theme.background)
            .text_color(theme.text)
            .on_action(
                cx.listener(|this, _: &CloseSelected, window, cx| this.close_selected(window, cx)),
            )
            .child(self.strip.clone())
            .child(
                cycle_on(div().w_full(), &self.strip)
                    .when_some(shown, |region, editor| region.child(editor)),
            )
            .when(selected.is_none(), |root| {
                root.child("No tabs. Everything was closed.")
            })
    }
}

/// The acceptance script: synthesized pointer and keyboard input and checks of the result.
struct Script {
    cx: AsyncApp,
    window: WindowHandle<Example>,
    failures: Vec<String>,
}

impl Script {
    async fn sleep(&self, duration: Duration) {
        self.cx.background_executor().timer(duration).await;
    }

    fn check(&mut self, ok: bool, what: &str) {
        println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        if !ok {
            self.failures.push(what.to_owned());
        }
    }

    fn send(&mut self, event: PlatformInput) {
        self.cx
            .update_window(self.window.into(), |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("the window is open");
    }

    fn mouse_move(&mut self, x: f32, y: f32, pressed: Option<MouseButton>) {
        self.send(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x), px(y)),
            pressed_button: pressed,
            modifiers: Modifiers::none(),
        }));
    }

    fn button(&mut self, x: f32, y: f32, button: MouseButton, down: bool) {
        let position = point(px(x), px(y));
        self.send(if down {
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers: Modifiers::none(),
                click_count: 1,
                first_mouse: false,
            })
        } else {
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers: Modifiers::none(),
                click_count: 1,
            })
        });
    }

    async fn click(&mut self, x: f32, y: f32, button: MouseButton) {
        self.mouse_move(x, y, None);
        self.sleep(Duration::from_millis(80)).await;
        self.button(x, y, button, true);
        self.button(x, y, button, false);
        self.sleep(Duration::from_millis(150)).await;
    }

    async fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.mouse_move(from.0, from.1, None);
        self.sleep(Duration::from_millis(80)).await;
        self.button(from.0, from.1, MouseButton::Left, true);
        self.mouse_move(from.0 + 6.0, from.1, Some(MouseButton::Left));
        self.sleep(Duration::from_millis(120)).await;
        self.mouse_move(to.0, to.1, Some(MouseButton::Left));
        self.sleep(Duration::from_millis(250)).await;
        self.button(to.0, to.1, MouseButton::Left, false);
        self.sleep(Duration::from_millis(250)).await;
    }

    async fn press(&mut self, key: &str) {
        let keystroke = Keystroke::parse(key).expect("a keystroke");
        self.send(PlatformInput::KeyDown(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        }));
        self.send(PlatformInput::KeyUp(KeyUpEvent { keystroke }));
        self.sleep(Duration::from_millis(200)).await;
    }

    fn order(&self) -> Vec<String> {
        self.window
            .read_with(&self.cx, |example, cx| {
                example
                    .strip
                    .read(cx)
                    .specs()
                    .iter()
                    .map(|tab| tab.id.to_string())
                    .collect()
            })
            .expect("the window is open")
    }

    fn selected(&self) -> Option<String> {
        self.window
            .read_with(&self.cx, |example, cx| {
                example
                    .strip
                    .read(cx)
                    .selected_id()
                    .map(ToString::to_string)
            })
            .expect("the window is open")
    }

    /// `(text, caret, focused)` of the editor of tab `id`.
    fn editor(&self, id: &str) -> Option<(String, usize, bool)> {
        let mut cx = self.cx.clone();
        self.window
            .update(&mut cx, |example, window, cx| {
                let (_, editor) = example
                    .editors
                    .iter()
                    .find(|(editor_id, _)| editor_id.as_str() == id)?;
                let editor = editor.read(cx);
                Some((
                    editor.text.clone(),
                    editor.caret,
                    editor.focus.is_focused(window),
                ))
            })
            .expect("the window is open")
    }

    async fn run(&mut self) {
        self.sleep(Duration::from_millis(600)).await;
        // Chips are 116 px apart from x = 12; the strip's row is at y = 12..36.
        let chip = |ix: usize| (12.0 + 56.0 + 116.0 * ix as f32, 24.0);
        let slot = |ix: usize| (12.0 + 116.0 * ix as f32 + 99.0, 24.0);

        let start_notes = self.editor("notes");
        let start_todo = self.editor("todo");
        let start_log = self.editor("log");
        self.check(
            self.order() == ["notes", "todo", "log"] && self.selected().as_deref() == Some("todo"),
            "three tabs, the second selected",
        );
        self.check(
            start_todo
                .as_ref()
                .is_some_and(|(_, caret, focused)| *caret == 3 && *focused),
            "the selected editor is focused with its own caret",
        );

        // Drag the first tab over the third.
        self.drag(chip(0), chip(2)).await;
        self.check(
            self.order() == ["todo", "log", "notes"],
            "dragging the first tab over the third reorders it",
        );
        self.check(
            self.selected().as_deref() == Some("todo"),
            "the selection followed its tab, not its index",
        );
        self.check(
            self.editor("todo") == start_todo,
            "reordering kept the selected editor's text, caret and focus",
        );

        // Close an inactive tab (middle click on "log", now second).
        self.click(chip(1).0, chip(1).1, MouseButton::Middle).await;
        self.check(
            self.order() == ["todo", "notes"],
            "middle-click closed the inactive tab",
        );
        self.check(
            self.selected().as_deref() == Some("todo"),
            "closing an inactive tab keeps the selected tab",
        );
        self.check(
            self.editor("log").is_none(),
            "the closed tab's editor is gone",
        );
        self.check(
            self.editor("todo") == start_todo,
            "the selected editor kept its state",
        );

        // Ctrl+Tab cycles with wrap; focus follows into the new content.
        self.press("ctrl-tab").await;
        self.check(
            self.selected().as_deref() == Some("notes"),
            "ctrl-tab selects the next tab",
        );
        self.check(
            self.editor("notes").is_some_and(|(_, _, focused)| focused),
            "focus moved into the newly selected editor",
        );
        self.press("ctrl-tab").await;
        self.check(
            self.selected().as_deref() == Some("todo"),
            "ctrl-tab wraps around",
        );
        self.press("ctrl-shift-tab").await;
        self.check(
            self.selected().as_deref() == Some("notes"),
            "ctrl-shift-tab wraps backwards",
        );
        self.press("ctrl-tab").await;

        // Close the selected tab through its close slot: NearestIndex selects the survivor.
        self.click(slot(0).0, slot(0).1, MouseButton::Left).await;
        self.check(
            self.order() == ["notes"],
            "the close slot closed the selected tab",
        );
        self.check(
            self.selected().as_deref() == Some("notes"),
            "the policy's survivor is selected",
        );
        self.check(
            self.editor("notes").is_some_and(|(text, caret, focused)| {
                Some((text, caret)) == start_notes.clone().map(|(text, caret, _)| (text, caret))
                    && focused
            }),
            "the surviving editor kept its text and caret and received focus",
        );
        let _ = start_log;
    }
}

fn main() {
    let script = std::env::var_os("TAB_STRIP_SCRIPT").is_some_and(|value| value == "1");
    application().run(move |cx: &mut App| {
        bind_keys(cx);
        cx.bind_keys([KeyBinding::new("ctrl-w", CloseSelected, None)]);
        let bounds = Bounds::centered(None, size(px(720.), px(280.)), cx);
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
            cx.spawn(async move |cx| {
                let mut script = Script {
                    cx: cx.clone(),
                    window,
                    failures: Vec::new(),
                };
                script.run().await;
                let code = if script.failures.is_empty() {
                    println!("tab_strip: all checks passed");
                    0
                } else {
                    println!("tab_strip: {} check(s) failed", script.failures.len());
                    1
                };
                std::process::exit(code);
            })
            .detach();
        }
        cx.activate(true);
    });
}
