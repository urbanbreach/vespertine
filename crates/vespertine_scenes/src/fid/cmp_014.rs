//! FID-CMP-014: a code block host with line numbers inside a padded surface.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};
use vespertine_components::code::CodeBlock;
use vespertine_richtext::text_root;
use vespertine_theme::document::DocumentTheme;

struct Cmp014;

impl Scene for Cmp014 {
    fn size(&self) -> (f32, f32) {
        (600.0, 220.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        // The dark base with the text token #eeeeee and Liberation Mono as the monospace family.
        let mut theme = DocumentTheme::dark();
        theme.text = vespertine::rgb(0xeeeeee);
        theme.font_mono = "Liberation Mono".into();
        let block = CodeBlock::new("code", "alpha\nbeta\ngamma")
            .theme(theme)
            .show_line_numbers(true)
            .font_family("Liberation Mono")
            .font_weight(vespertine::FontWeight::NORMAL)
            .text_size(px(14.))
            .line_height(px(22.))
            .text_color(hex(0xeeeeee));
        text_root(
            div().size_full().relative().bg(hex(0x181818)).child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .w(px(560.))
                    .h(px(144.))
                    .p(px(12.))
                    .rounded(px(8.))
                    .bg(hex(0x202020))
                    .child(block),
            ),
        )
        .into_any_element()
    }
}

register_scene!("FID-CMP-014", || Cmp014);
