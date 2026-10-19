//! FID-TXT-007: a one-line JSON code view colored through the syntax palette with theme
//! overrides; unclassified gaps (none here) share the metrics of colored text.

use crate::common::hex;
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{AnyElement, App, IntoElement, ParentElement, Styled, Window, div, px};
use vespertine_richtext::syntax::{CodeView, HighlightCache, RetainedHighlight, SyntaxColors};

const SOURCE: &str = r#"{"enabled":true,"count":2}"#;

/// The palette of the scene: THM-002 dark defaults with the spec's overrides and the plain
/// foreground #EEEEEE.
fn palette() -> SyntaxColors {
    SyntaxColors::dark()
        .with_plain(hex(0xEEEEEE))
        .with_category("string", hex(0x44CC88))
        .with_category("property", hex(0xFFAA44))
        .with_category("boolean", hex(0xFFAA44))
        .with_category("number", hex(0xFFAA44))
        .with_category("punctuation", hex(0xEEEEEE))
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
        let view = CodeView::new(self.source.clone())
            .language("json")
            .retained(&mut self.retained, &self.cache)
            .colors(palette())
            .font_family("Liberation Mono")
            .font_size(px(14.))
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
