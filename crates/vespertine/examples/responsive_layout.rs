//! Composable responsive layout (LAY-003).
//!
//! Run with: `cargo run -p vespertine --example responsive_layout`
//!
//! A row with 12 pixel gaps has fixed 100 pixel cells at both ends and a
//! flexible center. At 760 pixels wide the center is 536 pixels; resize the
//! window and the center shrinks while the side cells keep their size and stay
//! on screen, and the center's text truncates with an ellipsis instead of
//! pushing its neighbours away. Below the row, a three column grid shares its
//! width equally after the gaps (246 2/3 pixels each at 760 pixels).

#[path = "example_support/fonts.rs"]
mod example_support;

use vespertine::{
    App, Bounds, Context, Div, Window, WindowBounds, WindowOptions, container_query, div,
    prelude::*, px, rgb, size,
};
use vespertine_platform::application;

struct ResponsiveLayout;

fn cell(label: &'static str, color: u32) -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(color))
        .rounded_md()
        .child(label)
}

impl Render for ResponsiveLayout {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        container_query(|container, _window, _cx| {
            let center_width = (container.width - px(224.)).max(px(0.));
            div()
                .size_full()
                .flex()
                .flex_col()
                .gap_4()
                .bg(rgb(0x1e1e2e))
                .text_color(rgb(0xcdd6f4))
                .text_sm()
                .child(
                    div()
                        .h(px(120.))
                        .w_full()
                        .flex()
                        .gap(px(12.))
                        .child(cell("100 px", 0x585b70).w(px(100.)).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .bg(rgb(0x89b4fa))
                                .text_color(rgb(0x1e1e2e))
                                .rounded_md()
                                .p_2()
                                .child(
                                    div()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .overflow_hidden()
                                        .child(format!(
                                            "Flexible center: {} px. This text truncates with an \
                                             ellipsis instead of pushing the side cells off screen.",
                                            f32::from(center_width)
                                        )),
                                ),
                        )
                        .child(cell("100 px", 0x585b70).w(px(100.)).flex_none()),
                )
                .child(
                    div()
                        .w_full()
                        .grid()
                        .grid_cols(3)
                        .gap(px(10.))
                        .child(cell("1/3", 0xa6e3a1).h(px(60.)))
                        .child(cell("1/3", 0xf9e2af).h(px(60.)))
                        .child(cell("1/3", 0xf38ba8).h(px(60.))),
                )
        })
    }
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(760.), px(300.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| ResponsiveLayout),
        )
        .unwrap();
        cx.activate(true);
    });
}
