//! FID-THM-002: six swatches filled with document theme tokens resolved from theme values.

use crate::scene::{Scene, register_scene};
use serde_json::json;
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px, rgb};
use vespertine_theme::document::DocumentTheme;

struct Thm002;

impl Scene for Thm002 {
    fn size(&self) -> (f32, f32) {
        (460.0, 100.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let red_accent = DocumentTheme::from_value(Some(&json!({"accent": "#FF0000"})));
        let fills = [
            DocumentTheme::dark().bg,
            DocumentTheme::light().bg,
            DocumentTheme::dark().accent,
            red_accent.accent,
            DocumentTheme::dark().text,
            DocumentTheme::light().accent,
        ];
        let mut root = div().relative().size_full().bg(rgb(0x202020));
        for (index, fill) in fills.into_iter().enumerate() {
            root = root.child(
                div()
                    .absolute()
                    .left(px(20.0 + 70.0 * index as f32))
                    .top(px(30.0))
                    .w(px(40.0))
                    .h(px(40.0))
                    .bg(fill),
            );
        }
        root.into_any_element()
    }
}

register_scene!("FID-THM-002", || Thm002);
