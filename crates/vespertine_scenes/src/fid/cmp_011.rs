//! FID-CMP-011: a Markdown document with a heading, a paragraph and a block quote.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};
use vespertine_components::markdown_doc::Markdown;
use vespertine_richtext::text_root;
use vespertine_theme::document::DocumentTheme;

struct Cmp011;

/// The dark base with the scene's token values.
pub fn theme() -> DocumentTheme {
    let mut theme = DocumentTheme::dark();
    theme.text = vespertine::rgb(0xeeeeee);
    theme.accent = vespertine::rgb(0x8080ff);
    theme.border = vespertine::Rgba {
        r: 1.,
        g: 1.,
        b: 1.,
        a: 0x14 as f32 / 255.,
    };
    theme.font_sans = "Noto Sans".into();
    theme.font_mono = "Liberation Mono".into();
    theme
}

impl Scene for Cmp011 {
    fn size(&self) -> (f32, f32) {
        (640.0, 460.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        text_root(
            div().size_full().relative().bg(hex(0x181818)).child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .w(px(600.))
                    .child(
                        Markdown::new("doc", "# Document\n\nA short paragraph.\n\n> Quoted text\n")
                            .theme(theme()),
                    ),
            ),
        )
        .into_any_element()
    }
}

register_scene!("FID-CMP-011", || Cmp011);
