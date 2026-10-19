//! Background work publishing into a view while an independent input counter keeps updating.
//!
//! The upper panel counts clicks and space-bar presses. The lower panel shows a value that a
//! background task publishes after sleeping for three seconds. The counter responds immediately
//! during the whole wait, because the sleep happens on the background executor and only the
//! finished result returns to the UI executor.

#[path = "example_support/fonts.rs"]
mod example_support;
#[path = "example_support/reactive_background.rs"]
mod reactive_background;

use std::time::Duration;

use vespertine::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use vespertine_platform::application;

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        reactive_background::bind_keys(cx);
        let bounds = Bounds::centered(None, size(px(260.), px(140.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    reactive_background::ReactiveDemo::new(Duration::from_secs(3), 42, window, cx)
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
