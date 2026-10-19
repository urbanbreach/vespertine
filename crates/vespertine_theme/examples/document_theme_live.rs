//! THM-002 A5: changes a code view's text color and line height at runtime through the
//! document theme value. Click the window (or wait three seconds) to apply the next override;
//! the next frame shows the new colors and row positions without rebuilding anything.

#[path = "example_support/code_rows.rs"]
mod code_rows;

use std::time::Duration;

use code_rows::{CodeRows, steps};
use vespertine::{
    App, AppContext as _, Bounds, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Window, WindowBounds, WindowOptions, div, px, size,
};
use vespertine_platform::application;
use vespertine_theme::document::DocumentTheme;

struct Live {
    rows: Entity<CodeRows>,
    step: usize,
    _timer: vespertine::Task<()>,
}

impl Live {
    fn advance(&mut self, cx: &mut Context<Self>) {
        let steps = steps();
        self.step = (self.step + 1) % steps.len();
        let (label, value) = &steps[self.step];
        println!("applying theme override: {label}");
        let theme = DocumentTheme::from_value(Some(value));
        self.rows.update(cx, |rows, cx| rows.set_theme(theme, cx));
        cx.notify();
    }
}

impl Render for Live {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .on_mouse_down(
                vespertine::MouseButton::Left,
                cx.listener(|this, _, _, cx| this.advance(cx)),
            )
            .child(self.rows.clone())
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.), px(400.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                let (_, first) = &steps()[0];
                let rows = cx.new(|_| CodeRows::new(DocumentTheme::from_value(Some(first))));
                cx.new(|cx| Live {
                    rows,
                    step: 0,
                    _timer: cx.spawn_in(window, async move |this, cx| {
                        loop {
                            cx.background_executor().timer(Duration::from_secs(3)).await;
                            if this
                                .update(cx, |this: &mut Live, cx| this.advance(cx))
                                .is_err()
                            {
                                return;
                            }
                        }
                    }),
                })
            },
        )
        .unwrap();
        cx.activate(true);
    });
}
