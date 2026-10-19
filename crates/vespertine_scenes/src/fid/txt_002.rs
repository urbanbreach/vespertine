//! FID-TXT-002: fallback fonts preserve requested style (spec TXT-002).

use std::sync::Arc;

use crate::common::{WHITE, hex};
use crate::scene::{Scene, register_scene};
use vespertine::{
    AnyElement, App, FontFeatures, FontWeight, IntoElement, ParentElement, Styled, Window, div, px,
};

/// First installed entry of the fallback stack on the verification machine.
const FIRST_STACK_FAMILY: &str = "Adwaita Sans";
const MISSING_FAMILY: &str = "No Such Family";
/// Installed family with static bold italic faces; shows a request satisfied by a bold italic face.
const STATIC_BOLD_ITALIC_FAMILY: &str = "Liberation Sans";
const SAMPLE: &str = "0123456789 Bold fallback";

pub struct FallbackStyleScene;

impl FallbackStyleScene {
    pub fn new() -> Self {
        Self
    }
}

/// Bold italic 20 px sample with tabular numerals, in `family`, at (20, `top`).
fn sample(family: &'static str, top: f32) -> impl IntoElement {
    div()
        .absolute()
        .left(px(20.0))
        .top(px(top))
        .whitespace_nowrap()
        .font_family(family)
        .font_weight(FontWeight::BOLD)
        .italic()
        .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
        .text_size(px(20.0))
        .line_height(px(28.0))
        .text_color(hex(0x202020))
        .child(SAMPLE)
}

impl Scene for FallbackStyleScene {
    fn size(&self) -> (f32, f32) {
        (600.0, 220.0)
    }

    fn frame(&mut self, _f: u64, _window: &mut Window, _cx: &mut App) -> AnyElement {
        div()
            .relative()
            .size_full()
            .bg(hex(WHITE))
            .child(sample(FIRST_STACK_FAMILY, 20.0))
            .child(sample(MISSING_FAMILY, 90.0))
            .child(sample(STATIC_BOLD_ITALIC_FAMILY, 150.0))
            .into_any_element()
    }
}

register_scene!("FID-TXT-002", FallbackStyleScene::new);
