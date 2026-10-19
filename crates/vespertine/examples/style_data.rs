//! ELM-006: serializable layout and interactive styling.
//!
//! Every panel is described by JSON (`examples/style_data/coverage.json`, which exercises every
//! property of the style inventory) and applied through `HostStyle`. Click a panel title to
//! replace the panel's style with one that omits properties: omitted properties revert to the
//! upstream defaults (a "removal update"), and the empty style resets everything.

#[path = "example_support/fonts.rs"]
mod example_support;

use serde_json::Value;
use vespertine::{
    App, Bounds, Context, HostStyle, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb,
    size,
};
use vespertine_platform::application;

const COVERAGE: &str = include_str!("style_data/coverage.json");

struct Panel {
    title: String,
    container: Value,
    child: HostStyle,
    removal: Value,
    style: HostStyle,
    removed: bool,
}

struct StyleDataExample {
    panels: Vec<Panel>,
}

impl StyleDataExample {
    fn new() -> Self {
        let coverage: Value = serde_json::from_str(COVERAGE).expect("coverage.json is valid JSON");
        let panels = coverage["panels"]
            .as_array()
            .expect("panels")
            .iter()
            .map(|panel| {
                let container = panel["container"].clone();
                let style = HostStyle::from_json(&container, "example.container")
                    .expect("coverage container style is valid");
                Panel {
                    title: panel["title"].as_str().unwrap_or_default().to_owned(),
                    child: HostStyle::from_json(&panel["child"], "example.child")
                        .expect("coverage child style is valid"),
                    // A panel without a replacement style is reset to the empty style.
                    removal: panel.get("removal").cloned().unwrap_or(Value::Null),
                    container,
                    style,
                    removed: false,
                }
            })
            .collect();
        Self { panels }
    }

    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        let panel = &mut self.panels[index];
        panel.removed = !panel.removed;
        let next = if panel.removed {
            panel.removal.clone()
        } else {
            panel.container.clone()
        };
        // `replace` reports whether anything changed: an equal style needs no repaint.
        let mut next_style = panel.style.clone();
        match next_style.replace(&next, "example.replace") {
            Ok(true) => {
                panel.style = next_style;
                cx.notify();
            }
            Ok(false) => {}
            Err(error) => eprintln!("{error}"),
        }
    }
}

impl Render for StyleDataExample {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .id("root")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(0x0b0f19))
            .text_color(rgb(0xe5e7eb))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(14.));
        for (index, panel) in self.panels.iter().enumerate() {
            let mut container = panel.style.apply(div().id(("panel", index)));
            for label in ["A", "B", "C"] {
                container =
                    container.child(panel.child.apply(div().id((label, index)).child(label)));
            }
            root = root.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .id(("title", index))
                            .cursor_pointer()
                            .text_size(px(12.))
                            .text_color(rgb(0x94a3b8))
                            .child(format!(
                                "{}{}",
                                panel.title,
                                if panel.removed {
                                    "  [style removed: click to restore]"
                                } else {
                                    "  [click: removal update]"
                                }
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(index, cx))),
                    )
                    .child(container),
            );
        }
        root
    }
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(720.), px(820.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| StyleDataExample::new()),
        )
        .unwrap();
        cx.activate(true);
    });
}
