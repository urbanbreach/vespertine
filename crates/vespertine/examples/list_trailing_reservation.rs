//! LAY-002 A1: trailing viewport reservation.
//!
//! The list reserves one viewport (inset 24px) for the whole suffix. Click "Append row" to add
//! rows: they first consume the reservation, so the scroll extent stays at the reserved height
//! until the natural content exceeds it, and no transient scrollable gap appears.

use vespertine::{
    App, Bounds, Context, ListAlignment, ListState, MouseButton, Render, Window, WindowBounds,
    WindowOptions, div, list, prelude::*, px, rgb, size,
};
use vespertine_platform::application;

const ROW_HEIGHT: f32 = 40.;

struct ReservationDemo {
    state: ListState,
    rows: usize,
}

impl ReservationDemo {
    fn new() -> Self {
        let state = ListState::new(2, ListAlignment::Top, px(200.));
        state.set_trailing_reservation(Some((0, px(24.))));
        Self { state, rows: 2 }
    }
}

impl Render for ReservationDemo {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let extent = self.state.logical_scroll_top_and_extent();
        let max_scroll = self.state.max_offset_for_scrollbar().y;
        let append = div()
            .px_2()
            .py_1()
            .bg(rgb(0x303030))
            .child("Append row")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.state.splice(this.rows..this.rows, 1);
                    this.rows += 1;
                    cx.notify();
                }),
            );

        div()
            .size_full()
            .bg(rgb(0x202020))
            .text_color(rgb(0xFFFFFF))
            .flex()
            .flex_col()
            .p_4()
            .gap_2()
            .child(div().flex().flex_row().gap_2().child(append).child(format!(
                "rows {} | content {:.0}px | max scroll {:.0}px | filled {}",
                self.rows,
                extent.content_height.as_f32(),
                max_scroll.as_f32(),
                self.state.trailing_reservation_filled(),
            )))
            .child(
                list(self.state.clone(), |ix, _, _| {
                    div()
                        .h(px(ROW_HEIGHT))
                        .w_full()
                        .px_2()
                        .flex()
                        .items_center()
                        .bg(rgb(if ix % 2 == 0 { 0x505050 } else { 0x707070 }))
                        .child(format!("Row {ix}"))
                        .into_any()
                })
                .flex_1(),
            )
    }
}

fn main() {
    application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(420.), px(360.)), cx);
        cx.open_window(
            WindowOptions {
                focus: true,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| ReservationDemo::new()),
        )
        .unwrap();
        cx.activate(true);
    });
}
