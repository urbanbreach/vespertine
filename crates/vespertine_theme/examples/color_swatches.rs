//! THM-001 A8 / FID-THM-001 and FID-THM-002: paints the swatch scenes.
//!
//! `color_swatches` paints FID-THM-001 through an element style color (`ColorPath::ElementStyle`).
//! `color_swatches theme` paints the same scene through a component theme override field
//! (the `bg` token of a document theme); the pixels are identical.
//! `color_swatches tokens` paints FID-THM-002 from resolved theme tokens.

#[path = "example_support/swatches.rs"]
mod swatches;

use swatches::{ColorPath, SwatchScene};
use vespertine::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, px, size};
use vespertine_platform::application;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    application().run(move |cx: &mut App| {
        let scene = match mode.as_str() {
            "theme" => SwatchScene::thm_001(ColorPath::ThemeOverride),
            "tokens" => SwatchScene::thm_002(),
            _ => SwatchScene::thm_001(ColorPath::ElementStyle),
        };
        let bounds = Bounds::centered(None, size(px(scene.window.0), px(scene.window.1)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| scene),
        )
        .unwrap();
        cx.activate(true);
    });
}
