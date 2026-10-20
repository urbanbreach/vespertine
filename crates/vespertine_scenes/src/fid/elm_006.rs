//! FID-ELM-006: serializable layout and interactive styling, built from style data.
//!
//! The card and the three controls are described as JSON and applied through `HostStyle`. After
//! the first frame a Tab key press is dispatched and focus moves to the First and then to the
//! Second control, so the last input is the keyboard: the other controls show the keyboard-focus
//! dim and the Second one its focus-visible ring.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use vespertine::private::serde_json::{Value, json};
use vespertine::{
    AnyElement, App, FocusHandle, HostStyle, InteractiveElement, IntoElement, KeyDownEvent,
    Keystroke, ParentElement, PlatformInput, Styled, Window, div,
};

const LABELS: [&str; 3] = ["First", "Second", "Third"];

struct Elm006 {
    handles: Option<[FocusHandle; 3]>,
    frames: u32,
    card: HostStyle,
    controls: [HostStyle; 3],
}

fn style(value: Value) -> HostStyle {
    HostStyle::from_json(&value, "FID-ELM-006").expect("scene styles are valid")
}

impl Elm006 {
    fn new() -> Self {
        let card = style(json!({
            "position": "absolute", "left": 24, "top": 24, "width": 300, "height": 120,
            "borderRadius": 12, "borderWidth": 2, "borderStyle": "solid", "borderColor": "#808080",
            "padding": 16,
            "background": {
                "type": "linear-gradient", "angle": 90, "colorSpace": "oklab",
                "stops": [
                    { "color": "#FF0000", "position": 0 },
                    { "color": "#0000FF", "position": 1 },
                ],
            },
            "boxShadow": [
                { "offsetX": 0, "offsetY": 1, "blurRadius": 4, "spreadRadius": 0, "color": "#00000040", "inset": true },
                { "offsetX": 0, "offsetY": 4, "blurRadius": 12, "spreadRadius": 0, "color": "#00000033" },
            ],
        }));
        let control = |left: u32| {
            style(json!({
                "position": "absolute", "left": left, "top": 180, "width": 140, "height": 44,
                "backgroundColor": "#303030", "borderRadius": 6,
                "display": "flex", "alignItems": "center", "justifyContent": "center",
                "color": "#FFFFFF", "fontFamily": "Noto Sans", "fontSize": 14, "fontWeight": 400,
                "lineHeight": 20,
                "focusVisible": { "outlineWidth": 2, "outlineColor": "#60A5FA", "outlineOffset": 2 },
            }))
        };
        Self {
            handles: None,
            frames: 0,
            card,
            controls: [control(24), control(176), control(328)],
        }
    }
}

impl Scene for Elm006 {
    fn size(&self) -> (f32, f32) {
        (720.0, 400.0)
    }

    fn frame(&mut self, _f: u64, window: &mut Window, cx: &mut App) -> AnyElement {
        let handles = self
            .handles
            .get_or_insert_with(|| std::array::from_fn(|_| cx.focus_handle().tab_stop(true)))
            .clone();
        // Once the first frame has registered the tab stops, press Tab and move focus to the
        // Second control. Later frames only render.
        self.frames += 1;
        if self.frames == 2 {
            let handles = handles.clone();
            window.defer(cx, move |window, cx| {
                window.dispatch_event(
                    PlatformInput::KeyDown(KeyDownEvent {
                        keystroke: Keystroke::parse("tab").expect("valid keystroke"),
                        is_held: false,
                        prefer_character_input: false,
                    }),
                    cx,
                );
                window.focus(&handles[0], cx);
                window.focus(&handles[1], cx);
            });
        }
        let mut root = div()
            .size_full()
            .relative()
            .bg(hex(0x181818))
            .child(self.card.apply(div().id("card")));
        for (index, label) in LABELS.iter().enumerate() {
            root = root.child(
                self.controls[index]
                    .apply(div().id(*label))
                    .track_focus(&handles[index])
                    .child(*label),
            );
        }
        root.into_any_element()
    }
}

register_scene!("FID-ELM-006", Elm006::new);
