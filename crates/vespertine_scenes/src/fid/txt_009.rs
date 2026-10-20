//! FID-TXT-009: a search highlight with an active match.

use crate::common::{SANS, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};
use vespertine_richtext::search::{HighlightDeclaration, HighlightSpec, default_hex_color};
use vespertine_richtext::{RichText, highlight, text_root};

struct Find;

impl Scene for Find {
    fn size(&self) -> (f32, f32) {
        (560.0, 180.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let spec = HighlightSpec::new("red")
            .with_radius(2.0)
            .with_color(default_hex_color("#F0C04066").expect("color"))
            .with_active_color(default_hex_color("#F08040AA").expect("color"))
            .with_active_index(Some(1));
        let block = div()
            .absolute()
            .left(px(20.))
            .top(px(20.))
            .w(px(520.))
            .font_family(SANS)
            .text_size(px(16.))
            .line_height(px(24.))
            .text_color(hex(0xeeeeee))
            .child(RichText::new("find", "Find red, red and RED."));
        text_root(
            div()
                .size_full()
                .relative()
                .bg(hex(0x181818))
                .child(highlight(
                    "find-highlight",
                    HighlightDeclaration::from_spec(spec),
                    block,
                )),
        )
        .into_any_element()
    }
}

register_scene!("FID-TXT-009", || Find);
