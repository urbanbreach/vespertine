//! FID-TXT-007: a one-line JSON code block colored through the syntax palette with theme
//! overrides; unclassified gaps (none here) share the metrics of colored text.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px, rgb};
use vespertine_components::code::CodeBlock;
use vespertine_richtext::syntax::{HighlightCache, LanguageHint, RetainedHighlight};
use vespertine_theme::document::DocumentTheme;

const SOURCE: &str = r#"{"enabled":true,"count":2}"#;

/// The theme of the scene: THM-002 dark defaults with the spec's syntax overrides and the plain
/// foreground #EEEEEE.
fn theme() -> DocumentTheme {
    let mut theme = DocumentTheme::dark();
    theme.text = rgb(0xEEEEEE);
    theme.syntax.string = rgb(0x44CC88);
    theme.syntax.property = rgb(0xFFAA44);
    theme.syntax.boolean = rgb(0xFFAA44);
    theme.syntax.number = rgb(0xFFAA44);
    theme.syntax.punctuation = rgb(0xEEEEEE);
    theme
}

struct Txt007 {
    source: Arc<str>,
    cache: Arc<HighlightCache>,
    retained: RetainedHighlight,
}

impl Scene for Txt007 {
    fn size(&self) -> (f32, f32) {
        (520.0, 160.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        let hint = LanguageHint {
            fence_label: Some("json"),
            path: None,
        };
        let document = self
            .retained
            .resolve(&self.cache, &self.source, hint)
            .cloned();
        let view = CodeBlock::new("fid-txt-007", &*self.source)
            .language("json")
            .highlight(document)
            .theme(theme())
            .font_family("Liberation Mono")
            .text_size(px(14.))
            .line_height(px(22.))
            .absolute()
            .left(px(20.))
            .top(px(20.))
            .w(px(480.))
            .h(px(60.));
        div()
            .size_full()
            .bg(hex(0x181818))
            .child(view)
            .into_any_element()
    }
}

register_scene!("FID-TXT-007", || Txt007 {
    source: Arc::from(SOURCE),
    cache: HighlightCache::global(),
    retained: RetainedHighlight::new(),
});
