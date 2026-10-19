//! Declarative accessibility properties (A11Y-001).
//!
//! Run with: `cargo run -p vespertine --example a11y_declarative`
//!
//! The window exports a heading with a level, an image whose alternative text
//! becomes its label, and a button with selected and expanded state, all through
//! the DOM-style role tokens and hyphenated `aria-*` properties a declarative
//! host uses. Click the button to change the properties: the heading level
//! cycles from 1 to 3, the image alternative text changes and the button
//! toggles its expanded and selected state. With the environment variable
//! `A11Y_DUMP=1` the example marks accessibility active without an assistive
//! client and prints the accessibility debug tree after every frame, so you can
//! watch the properties update.

#[path = "example_support/fonts.rs"]
mod example_support;

use std::sync::Arc;

use vespertine::{
    App, Bounds, Context, ImageSource, RenderImage, SharedString, Window, WindowBounds,
    WindowOptions, apply_aria_properties, div, img, prelude::*, px, rgb, size, text,
};
use vespertine_platform::application;

struct Demo {
    level: usize,
    expanded: bool,
    image: Arc<RenderImage>,
    dump: bool,
}

impl Demo {
    fn new(window: &mut Window) -> Self {
        let dump = std::env::var_os("A11Y_DUMP").is_some();
        if dump {
            window.set_a11y_active_for_tests(true);
        }
        let swatch = image::RgbaImage::from_pixel(64, 64, image::Rgba([0x89, 0xb4, 0xfa, 0xff]));
        let frame = image::Frame::new(swatch);
        Self {
            level: 1,
            expanded: false,
            image: Arc::new(RenderImage::new(vec![frame])),
            dump,
        }
    }
}

impl Render for Demo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.dump {
            window.on_next_frame(|window, _| {
                if let Some(tree) = window.debug_a11y_tree_json() {
                    println!("{tree}");
                }
            });
        }

        let alt = if self.expanded {
            "A blue square, enlarged"
        } else {
            "A blue square"
        };
        let heading = serde_json::json!({
            "role": "heading",
            "aria-id": "heading",
            "aria-level": self.level,
            "aria-label": format!("Section level {}", self.level),
        });
        let picture = serde_json::json!({
            "role": "img",
            "aria-id": "picture",
            "alt": alt,
        });
        let button = serde_json::json!({
            "role": "button",
            "aria-id": "toggle",
            "aria-label": "Toggle details",
            "aria-expanded": self.expanded,
            "aria-selected": !self.expanded,
        });
        let size_of_picture = if self.expanded { px(96.) } else { px(48.) };

        div()
            .id("root")
            .role(vespertine::Role::Group)
            .aria_label("Declarative accessibility")
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_4()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child(apply_aria_properties(
                div()
                    .id("heading")
                    .text_xl()
                    .child(text!(format!("Heading, level {}", self.level))),
                heading.as_object().expect("an object"),
            ))
            .child(apply_aria_properties(
                img(ImageSource::Render(self.image.clone()))
                    .id("picture")
                    .size(size_of_picture),
                picture.as_object().expect("an object"),
            ))
            .child(
                apply_aria_properties(
                    div()
                        .id("toggle")
                        .focusable()
                        .tab_stop(true)
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0x585b70))
                        .cursor_pointer(),
                    button.as_object().expect("an object"),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.expanded = !this.expanded;
                    this.level = this.level % 3 + 1;
                    cx.notify();
                }))
                .child(text!(SharedString::from(if self.expanded {
                    "Collapse"
                } else {
                    "Expand"
                }))),
            )
    }
}

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Warn)
        .filter_module("vespertine", log::LevelFilter::Info)
        .init();
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(420.), px(320.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|_| Demo::new(window)),
        )
        .unwrap();
        cx.activate(true);
    });
}
