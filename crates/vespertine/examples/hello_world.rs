#[path = "example_support/fonts.rs"]
mod example_support;

use vespertine::{
    App, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions, div, prelude::*, px,
    rgb, size,
};
use vespertine_platform::application;

struct HelloWorld {
    text: SharedString,
}

impl Render for HelloWorld {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x505050))
            .size(px(500.0))
            .justify_center()
            .items_center()
            .shadow_lg()
            .border_1()
            .border_color(rgb(0x0000ff))
            .text_xl()
            .text_color(rgb(0xffffff))
            .child(format!("Hello, {}!", self.text))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .size_8()
                            .bg(vespertine::red())
                            .border_1()
                            .border_dashed()
                            .rounded_md()
                            .border_color(vespertine::white()),
                    )
                    .child(
                        div()
                            .size_8()
                            .bg(vespertine::green())
                            .border_1()
                            .border_dashed()
                            .rounded_md()
                            .border_color(vespertine::white()),
                    )
                    .child(
                        div()
                            .size_8()
                            .bg(vespertine::blue())
                            .border_1()
                            .border_dashed()
                            .rounded_md()
                            .border_color(vespertine::white()),
                    )
                    .child(
                        div()
                            .size_8()
                            .bg(vespertine::yellow())
                            .border_1()
                            .border_dashed()
                            .rounded_md()
                            .border_color(vespertine::white()),
                    )
                    .child(
                        div()
                            .size_8()
                            .bg(vespertine::black())
                            .border_1()
                            .border_dashed()
                            .rounded_md()
                            .rounded_md()
                            .border_color(vespertine::white()),
                    )
                    .child(
                        div()
                            .size_8()
                            .bg(vespertine::white())
                            .border_1()
                            .border_dashed()
                            .rounded_md()
                            .border_color(vespertine::black()),
                    ),
            )
    }
}

fn run_example() {
    application().run(|cx: &mut App| {
        if !example_support::load_fonts(cx) {
            return;
        }
        let bounds = Bounds::centered(None, size(px(500.), px(500.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| HelloWorld {
                    text: "World".into(),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}

fn main() {
    run_example();
}
