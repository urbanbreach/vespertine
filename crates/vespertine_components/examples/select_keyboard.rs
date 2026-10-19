//! CMP-005 A10: a native select operated with the keyboard only.
//!
//! Tab focuses the select; Down/Up (or Ctrl-N/Ctrl-P) open it on the selected option and step with
//! wraparound over the enabled options; Home/End jump; Enter or Space commit; Escape closes.

use vespertine::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window, WindowBounds, WindowOptions, div, px, rgb, size,
};
use vespertine_components::select::{Select, SelectOption};
use vespertine_platform::application;

struct Page {
    select: Entity<Select>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chosen = self
            .select
            .read(cx)
            .selected_id()
            .map_or("none".to_string(), |id| id.to_string());
        div()
            .size_full()
            .bg(rgb(0x181A1F))
            .text_color(rgb(0xF0F1F3))
            .p(px(24.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child("Tab to the select, then Down / Up / Home / End / Enter / Space / Escape")
            .child(self.select.clone())
            .child(format!("selected: {chosen}"))
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(560.), px(420.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| {
                let select = cx.new(|cx| {
                    Select::new(
                        "letters",
                        "Letter",
                        vec![
                            SelectOption::new("a", "Alpha").shortcut("A"),
                            SelectOption::new("b", "Beta").disabled(true),
                            SelectOption::new("c", "Gamma").secondary("third"),
                            SelectOption::new("d", "Delta"),
                        ],
                        cx,
                    )
                    .placeholder("Pick a letter")
                    .width(px(220.))
                });
                cx.new(|_| Page { select })
            },
        )
        .expect("open the window");
        cx.activate(true);
    });
}
