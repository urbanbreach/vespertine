//! Frosted-glass panel made from the window's own content (RND-003): a striped pattern is
//! painted, a rounded panel blurs everything painted before it, and the panel's tint, border
//! and text composite crisply above the blurred region.

#[path = "example_support/fonts.rs"]
mod example_support;

use vespertine::{
    App, Bounds, Context, Corners, IntoElement, Render, Window, WindowBounds, WindowOptions,
    canvas, div, fill, hsla, point, prelude::*, px, rgb, size,
};
use vespertine_platform::application;

struct BackdropBlurExample;

const PANEL_RADIUS: f32 = 28.0;

impl Render for BackdropBlurExample {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let stripes = canvas(
            |_, _, _| (),
            |bounds, _, window, _| {
                window.paint_quad(fill(bounds, rgb(0x101820)));
                let mut x = bounds.origin.x;
                while x < bounds.right() {
                    let stripe = Bounds {
                        origin: point(x, bounds.origin.y),
                        size: size(px(20.), bounds.size.height),
                    };
                    window.paint_quad(fill(stripe, rgb(0xF0C040)));
                    x += px(40.);
                }
            },
        )
        .absolute()
        .size_full();

        // Blur radius 16 logical pixels (device sigma 32 at scale factor 2).
        let blur = canvas(
            |_, _, _| (),
            |bounds, _, window, _| {
                window.paint_backdrop_blur(bounds, Corners::all(px(PANEL_RADIUS)), px(16.));
            },
        )
        .absolute()
        .size_full();

        let panel = div()
            .absolute()
            .left(px(160.))
            .top(px(80.))
            .w(px(320.))
            .h(px(220.))
            .child(blur)
            .child(
                div()
                    .absolute()
                    .size_full()
                    .rounded(px(PANEL_RADIUS))
                    .border_1()
                    .border_color(hsla(0., 0., 1., 0.5))
                    .bg(hsla(0., 0., 1., 0.10))
                    .child(
                        div()
                            .absolute()
                            .left(px(30.))
                            .top(px(40.))
                            .text_size(px(32.))
                            .text_color(rgb(0xFFFFFF))
                            .child("Frosted panel"),
                    ),
            );

        div()
            .relative()
            .size_full()
            .bg(rgb(0x101820))
            .child(stripes)
            .child(panel)
    }
}

fn main() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(640.), px(400.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| BackdropBlurExample),
        )
        .unwrap();
        cx.activate(true);
    });
}
