//! A minimal code-view stand-in that lays out one row per source line from the metrics of a
//! [`DocumentTheme`] (THM-002 A5). Shared by the `document_theme_live` example and its test.

#![allow(dead_code)]

use serde_json::{Value, json};
use vespertine::{
    Context, Element as _, IntoElement, ListAlignment, ListState, Render, Styled as _, Window, div,
    list, prelude::*, px,
};
use vespertine_theme::document::{DocumentTheme, LayoutIdentity};

pub const SOURCE: [&str; 12] = [
    "fn main() {",
    "    let theme = Theme::dark();",
    "    // override at runtime",
    "    for line in source.lines() {",
    "        render(line, &theme);",
    "    }",
    "    println!(\"done: {}\", 42);",
    "}",
    "",
    "struct Theme { text: Rgba }",
    "impl Theme {",
    "}",
];

/// Overrides the example cycles through; each replaces the previous theme value wholesale.
pub fn steps() -> Vec<(&'static str, Value)> {
    vec![
        ("dark defaults", json!({})),
        (
            "text #ffcc00, line height 30",
            json!({"text": "#ffcc00", "accent": "#ff3b30",
                   "metrics": {"codeLineHeight": 30, "codeTextSize": 16}}),
        ),
        (
            "light, line height 24",
            json!({"appearance": "light", "metrics": {"codeLineHeight": 24}}),
        ),
        (
            "text oklch(0.7 0.15 150), line height 40",
            json!({"text": "oklch(0.7 0.15 150)", "metrics": {"codeLineHeight": 40}}),
        ),
    ]
}

pub struct CodeRows {
    pub theme: DocumentTheme,
    state: ListState,
    identity: LayoutIdentity,
}

impl CodeRows {
    pub fn new(theme: DocumentTheme) -> Self {
        Self {
            theme,
            state: ListState::new(SOURCE.len(), ListAlignment::Top, px(0.)).measure_all(),
            identity: LayoutIdentity::new(),
        }
    }

    /// Replaces the theme value; the next frame paints the new colors and lays rows out with
    /// the new metrics.
    pub fn set_theme(&mut self, theme: DocumentTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }
}

impl Render for CodeRows {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.identity
            .remeasure_on_layout_change(&self.theme, &self.state);
        let theme = self.theme.clone();
        div().size_full().bg(theme.bg).child(
            list(self.state.clone(), move |index, _, _| {
                // A gutter chip and a separator row in the text and accent colors make the
                // painted theme observable in pixels; the text uses the mono family.
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .w_full()
                    .h(px(theme.metrics.code_line_height))
                    .border_b_1()
                    .border_color(theme.accent)
                    .child(div().w(px(6.)).h_full().bg(theme.text))
                    .child(
                        div()
                            .font_family(theme.font_mono.clone())
                            .text_size(px(theme.metrics.code_text_size))
                            .text_color(theme.text)
                            .child(SOURCE[index]),
                    )
                    .into_any()
            })
            .size_full(),
        )
    }
}
