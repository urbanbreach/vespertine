//! ELM-008: layout-neutral element outlines.
//!
//! Shows positive and negative offsets, independent corner radii, a translucent outline color
//! and zero-width removal: hovering a box refines its outline to width zero, which hides it
//! without moving anything. Every box keeps the same size and position whether or not its
//! outline is shown.

#[path = "example_support/fonts.rs"]
mod example_support;

use vespertine::{
    App, Bounds, Context, Div, Hsla, Outline, SharedString, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, rgb, rgba, size,
};
use vespertine_platform::application;

struct OutlineExample;

fn card(label: &'static str) -> Div {
    div()
        .w(px(120.))
        .h(px(80.))
        .flex()
        .items_center()
        .justify_center()
        .bg(rgb(0x202020))
        .text_color(rgb(0xffffff))
        .text_size(px(12.))
        .child(SharedString::from(label))
}

fn ring(color: impl Into<Hsla>, width: f32, offset: f32) -> Outline {
    Outline::new(px(width), color, px(offset))
}

impl Render for OutlineExample {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .p(px(40.))
            .flex()
            .flex_wrap()
            .gap(px(40.))
            // Positive offset: a gap between the box and the ring, rounded corners follow.
            .child(
                card("offset +3, radius 8")
                    .id("positive")
                    .rounded(px(8.))
                    .outline(ring(rgb(0x0066ff), 2., 3.)),
            )
            // Negative offset: the ring sits inside the box; the square top-left corner stays
            // square while the other corners shrink with the offset.
            .child(
                card("offset -5, mixed radii")
                    .id("negative")
                    .rounded_tr(px(8.))
                    .rounded_br(px(8.))
                    .rounded_bl(px(8.))
                    .outline(ring(rgb(0xff8000), 3., -5.)),
            )
            // A translucent color over the white window background.
            .child(
                card("translucent")
                    .id("translucent")
                    .rounded(px(24.))
                    .outline(ring(rgba(0xe0115f80), 6., 2.)),
            )
            // Zero-width removal: the hover refinement replaces the whole outline value.
            .child(
                card("hover: width 0")
                    .id("hover")
                    .rounded(px(8.))
                    .outline(ring(rgb(0x10b981), 4., 4.))
                    .hover(|style| style.outline(ring(rgb(0x10b981), 0., 4.))),
            )
            // Independent radii, all different.
            .child(
                card("four radii")
                    .id("radii")
                    .rounded_tl(px(0.))
                    .rounded_tr(px(16.))
                    .rounded_br(px(4.))
                    .rounded_bl(px(30.))
                    .outline(ring(rgb(0x8b5cf6), 2., 6.)),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(640.), px(420.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| OutlineExample),
        )
        .unwrap();
        cx.activate(true);
    });
}
