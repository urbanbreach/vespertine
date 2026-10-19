//! ELM-006 R20: keyboard-focus dim and focus-visible.
//!
//! Press Tab: the focused control shows its focus-visible outline and every other focusable
//! control is multiplied by an opacity of 0.4. Clicking a control focuses it without dimming
//! anything and without showing the outline. Focusing the text field and typing dims nothing.
//! The "Third" control opts out with `keyboardFocusDim: false`, and the "Dim" toggle switches
//! the window option off and on. Layout never changes.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::ops::Range;

use serde_json::json;
use vespertine::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle, HostStyle,
    KeyBinding, SharedString, UTF16Selection, Window, WindowBounds, WindowOptions, actions, canvas,
    div, prelude::*, px, size,
};
use vespertine_platform::application;

actions!(example, [Tab, TabPrev, Quit]);

struct Example {
    root: FocusHandle,
    controls: Vec<(FocusHandle, &'static str, HostStyle)>,
    toggle: (FocusHandle, HostStyle),
    field: FocusHandle,
    field_style: HostStyle,
    text: String,
}

fn control_style(extra: serde_json::Value) -> HostStyle {
    let mut style = json!({
        "width": 140, "height": 44, "backgroundColor": "#303030", "color": "#ffffff",
        "borderRadius": 6, "display": "flex", "alignItems": "center", "justifyContent": "center",
        "fontSize": 14, "lineHeight": 20, "cursor": "pointer",
        "hover": { "backgroundColor": "#3c3c3c" },
        "active": { "backgroundColor": "#505050" },
        "focusVisible": { "outlineWidth": 2, "outlineColor": "#60A5FA", "outlineOffset": 2 },
    });
    style
        .as_object_mut()
        .expect("object")
        .extend(extra.as_object().expect("object").clone());
    HostStyle::from_json(&style, "example").expect("valid style")
}

impl Example {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = cx.focus_handle();
        window.focus(&root, cx);
        let handle = |cx: &mut Context<Self>| cx.focus_handle().tab_stop(true);
        Self {
            root,
            controls: vec![
                (handle(cx), "First", control_style(json!({}))),
                (handle(cx), "Second", control_style(json!({}))),
                (
                    handle(cx),
                    "Third (opt out)",
                    control_style(json!({ "keyboardFocusDim": false })),
                ),
            ],
            toggle: (
                handle(cx),
                control_style(json!({ "backgroundColor": "#1d4ed8" })),
            ),
            field: handle(cx),
            field_style: control_style(json!({
                "width": 200, "backgroundColor": "#101010", "justifyContent": "start",
                "padding": 10, "cursor": "text",
            })),
            text: String::new(),
        }
    }

    fn on_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_next(cx);
    }

    fn on_tab_prev(&mut self, _: &TabPrev, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_prev(cx);
    }

    fn on_quit(&mut self, _: &Quit, _window: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }
}

impl Render for Example {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let field = self.field.clone();
        let dim_enabled = window.keyboard_focus_dim_enabled();
        let mut row = div().flex().flex_row().gap(px(12.));
        for (index, (handle, label, style)) in self.controls.iter().enumerate() {
            row = row.child(
                style
                    .apply(div().id(("control", index)))
                    .track_focus(handle)
                    .child(*label),
            );
        }
        div()
            .id("app")
            .track_focus(&self.root)
            .size_full()
            .bg(vespertine::rgb(0x181818))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .text_color(vespertine::rgb(0xffffff))
            .on_action(cx.listener(Self::on_tab))
            .on_action(cx.listener(Self::on_tab_prev))
            .on_action(cx.listener(Self::on_quit))
            .child(row)
            .child(
                self.field_style
                    .apply(div().id("field"))
                    .track_focus(&self.field)
                    .child(SharedString::from(if self.text.is_empty() {
                        "type here: a focused text field dims nothing".to_owned()
                    } else {
                        self.text.clone()
                    }))
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, cx| {
                                window.handle_input(
                                    &field,
                                    ElementInputHandler::new(bounds, view.clone()),
                                    cx,
                                );
                            },
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .child(
                self.toggle
                    .1
                    .apply(div().id("toggle"))
                    .track_focus(&self.toggle.0)
                    .on_click(cx.listener(|_, _, window, cx| {
                        let enabled = window.keyboard_focus_dim_enabled();
                        window.set_keyboard_focus_dim_enabled(!enabled);
                        cx.notify();
                    }))
                    .child(if dim_enabled { "Dim: on" } else { "Dim: off" }),
            )
    }
}

impl EntityInputHandler for Example {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let utf16: Vec<u16> = self.text.encode_utf16().collect();
        let range = range.start.min(utf16.len())..range.end.min(utf16.len());
        *adjusted_range = Some(range.clone());
        String::from_utf16(&utf16[range]).ok()
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.text.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.text.push_str(text);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, new_text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<vespertine::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<vespertine::Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: vespertine::Point<vespertine::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        cx.bind_keys([
            KeyBinding::new("tab", Tab, None),
            KeyBinding::new("shift-tab", TabPrev, None),
            KeyBinding::new("cmd-q", Quit, None),
        ]);
        let bounds = Bounds::centered(None, size(px(560.), px(260.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // The default; `false` switches the keyboard-focus dim off for the window.
                keyboard_focus_dim: true,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Example::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
