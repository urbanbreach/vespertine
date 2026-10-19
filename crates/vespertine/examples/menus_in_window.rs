//! Application menus rendered inside the window from `cx.get_menus()`.
//!
//! Linux has no global desktop menu service in the toolkit: `set_menus` only stores the
//! descriptors. This example renders an App/Edit/Window menu bar and popup from them, shows a
//! separator, a disabled item and the key equivalents currently bound, and activates items with
//! the mouse or with the same shortcut. Both paths reach the same action handler and append the
//! same line to the activation log printed on stdout. Edit items are enabled only while the
//! focused surface handles them: click the read-only viewer and Cut/Paste/Undo/Redo grey out.

#[path = "example_support/fonts.rs"]
mod example_support;
#[path = "example_support/menus_in_window.rs"]
mod menus_in_window;

use vespertine::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use vespertine_platform::application;

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        menus_in_window::install(cx);
        // Print every activation so mouse and keyboard paths can be compared on stdout.
        cx.observe_global::<menus_in_window::ActivationLog>(|cx| {
            if let Some(last) = cx.global::<menus_in_window::ActivationLog>().entries.last() {
                println!("activated: {last}");
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(620.), px(220.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| menus_in_window::MenuDemo::new(window, cx)),
        )
        .unwrap();
        cx.activate(true);
    });
}
